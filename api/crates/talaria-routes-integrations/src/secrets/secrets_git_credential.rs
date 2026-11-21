// /api/secrets/git-credential.
//
// THE SANDBOX'S WAY IN — where a handle could not otherwise reach. A handle
// substitutes at the MCP gateway, which covers every tool call an agent
// makes THROUGH Talaria. It does not cover the shell inside a workbench
// sandbox: a coding harness runs `git push` with its own bash tool, we are
// not in that path, and the handle goes out as literal text. So git asks us
// — its credential helper forwards the protocol, host and path here over
// the agent's own key, and what that buys is precise: the value never enters
// the model's context, never appears in command output, and is never
// written to disk. The model runs `git push`, and the push works.
//
// AGENT-AUTHENTICATED, NOT SESSION-AUTHENTICATED. The caller is a process
// in a container presenting the agent's own credential; a human's session
// cannot reach this route at all — there is nothing here a person needs
// that /api/secrets/reveal does not already do with an audit trail.
//
// THE DOOR'S ORDER, each step before anything the next one opens:
//   1. a PROVEN agent (its own key, never the org-wide legacy one) whose
//      Developer switch is on — the same check that grants the Workbench
//      at the gateway (`talaria_agent_auth::agent_is_developer`), so an agent
//      whose switch went off keeps no git door through a leftover grant row
//   2. on a REGISTERED git host, the repo git named must be on this agent's
//      grant list there, before the workspace store is asked: its answer is
//      a spend (audited, and it burns a one-shot), and a PAT pinned for
//      github.com must not answer for every repo it happens to reach. Nor
//      under another name: a secret spendable at a registered host answers
//      only through that host's grant check, so an ask for `api.github.com`,
//      for a registered origin's hostname without its port, or for another
//      host on the secret's list passes it over (`CredentialScope::withheld`)
//   3. the workspace store, then the host's own login for the granted repo:
//      on GitHub a token for this agent alone, carrying the agent's set on
//      the host (what an admin set, or the default) as far as the
//      installation holds it, re-checked after the mint against a grant
//      narrowed meanwhile (talaria-git-hosts)
//   4. one `git.credential` audit row per (agent, token) — on a mint, and on
//      the first serve of a token this agent had not been handed — never one
//      per helper call (git asks on every fetch and push)

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::{now_ms, refuse_legacy, require_agent};
use talaria_audit::{AuditEntry, try_log_audit};
use talaria_body::{optional_max_string_member, parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::credential::{self, Ask, HostRefusal, Resolution, SecretAnswer, StoreRead};
use talaria_git_hosts::{self as hosts, AgentCredential};
use talaria_session::secretbox_or_500;
use talaria_state::AppState;

/// The sentence a non-Developer agent gets, at this door and at the push
/// check beside it. It names the switch, because that is what an admin turns.
pub(crate) const NOT_A_DEVELOPER: &str = credential::NOT_A_DEVELOPER;

/// The headers every answer that carries a credential, or why there is none,
/// goes out with: nothing between here and git keeps it.
const NO_STORE: [(header::HeaderName, &str); 2] = [
    (
        header::CACHE_CONTROL,
        "no-store, no-cache, must-revalidate, private",
    ),
    (header::PRAGMA, "no-cache"),
];

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(gate) => return Ok(gate),
    };
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let host = match string_member(obj, "host", 1, 253) {
        Ok(h) => h,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let protocol = match optional_max_string_member(obj, "protocol", 20) {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let path = match optional_max_string_member(obj, "path", 400) {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };

    // HTTPS ONLY. Answering for `http` would hand a live credential to a
    // cleartext connection, and a helper that does that turns one
    // misconfiguration into an interception.
    if let Some(p) = &protocol
        && p != "https"
    {
        tracing::warn!(
            "[secrets] {} asked for a {p} credential for {host} — refused",
            caller.model
        );
        return Ok(house_error(StatusCode::BAD_REQUEST, "https only"));
    }

    // A PROVEN AGENT. `id` is None for a LEGACY org-wide key, which names
    // nobody — and a caller we cannot identify is one we cannot scope to a
    // grant, a secret, or an audit row.
    let Some(agent_id) = caller.id.clone() else {
        tracing::warn!(
            "[secrets] a legacy-key caller ({}) asked for a credential for {host} — refused",
            caller.model
        );
        return Ok(refuse_legacy(&caller, "A git credential")
            .unwrap_or_else(|| house_error(StatusCode::NOT_FOUND, "no credential for that host")));
    };

    // THE ORDER (talaria-git-hosts' `credential::resolve`, which an admin's
    // "Check token" walks too): the Developer switch, then the grant before
    // any credential is opened, then the workspace store (a spend: audited,
    // and it burns a one-shot), then the host's own login. A credential
    // somebody deliberately granted for this host is a more specific answer
    // than the registered git host's own login, and an operator who pinned
    // one expects it used — on a registered host, for the repos this agent
    // was granted there.
    let asked = path.as_deref().map(|p| format!(" {p}")).unwrap_or_default();
    let sb = secretbox_or_500(&state, "[secrets] credential read failed").await?;
    let ask = Ask {
        agent_id: &agent_id,
        agent_model: &caller.model,
        authority: &host,
        path: path.as_deref(),
    };
    let resolution = match credential::resolve(&state.pg, &sb, &ask, StoreRead::Spend).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[secrets] git credential failed", e)),
    };
    let served = match resolution {
        // A DEVELOPER AGENT. The switch is what grants the Workbench, and the
        // git door is the Workbench's: grant rows and secrets left behind
        // when it was turned off answer nothing.
        Resolution::NotDeveloper => {
            tracing::warn!(
                "[secrets] {} asked for a credential for {host} but is not a Developer Agent — refused",
                caller.model
            );
            return Ok(house_error(StatusCode::FORBIDDEN, NOT_A_DEVELOPER));
        }
        // THE GRANT, before any credential is opened. Reported to the
        // OPERATOR, never elaborated to the caller: which credentials exist
        // and which hosts they cover is a map of the workspace, and git only
        // needs to know it got nothing. The operator log DOES name the path
        // — the caller asked for it, an admin can see the grants anyway, and
        // "which repo was the agent stuck on" is otherwise a guessing game
        // (the 2026-09-14 hang needed the api logs AND the grant table to
        // reconstruct).
        Resolution::Ungranted { host_id, repo } => {
            match repo {
                Some(repo) => tracing::warn!(
                    "[secrets] {} has no grant for {repo} on git host {host_id} — no credential for {host}{asked}",
                    caller.model
                ),
                None => tracing::warn!(
                    "[secrets] {} asked git host {host_id} ({host}) for a credential without naming a repo — refused",
                    caller.model
                ),
            }
            return Ok(no_credential());
        }
        Resolution::Secret(SecretAnswer {
            name,
            login: Some(login),
            host_id,
            repo,
        }) => Served {
            username: login.username,
            password: login.password,
            name: name.clone(),
            source: Source::WorkspaceSecret {
                name,
                host_id,
                repo,
            },
        },
        // The login of the git host registered at the host git named
        // (talaria-git-hosts: on GitHub, an installation token scoped to the
        // one repo, filed under this agent, and narrowed to the agent's set
        // on the host) — the credential the agent pushes and calls the host's
        // API with, and the one that used to ride into the model's context
        // inside the clone URL.
        Resolution::Host(c) => Served {
            username: c.login.username.clone(),
            password: c.login.password.clone(),
            name: format!("{}:{}", c.host_id, c.repo),
            source: Source::GitHost(c),
        },
        Resolution::NoLogin { host_id, repo } => {
            tracing::warn!(
                "[secrets] git host {host_id} has no login to give for {repo} ({} asked)",
                caller.model
            );
            return Ok(no_credential());
        }
        // A GRANTED repo the host would give no login for: the agent's git
        // is told why, in the host's own sentence (GitHub refused the mint,
        // the token came back wider than asked, the installation holds none
        // of the set), because "no credential" alone sends it guessing. A
        // refusal names nothing about the workspace the grant check would
        // not already: the repo is the agent's own.
        Resolution::Refused {
            host_id,
            repo,
            refusal,
        } => {
            tracing::warn!(
                "[secrets] git host {host_id} gave {} no login for {repo}: {}",
                caller.model,
                refusal.message
            );
            record_refusal(
                &state,
                &caller.model,
                &agent_id,
                &host,
                &host_id,
                &repo,
                &refusal,
            )
            .await;
            return Ok((
                StatusCode::BAD_GATEWAY,
                NO_STORE,
                Json(json!({ "error": credential::helper_safe(&refusal.message) })),
            )
                .into_response());
        }
        // A probe's answer never reaches the door (it spends), and a spend
        // that opened nothing is no answer.
        Resolution::Secret(SecretAnswer { login: None, .. }) | Resolution::Unanswered => {
            tracing::warn!(
                "[secrets] {} has no credential allowed for {host}{asked} (a workspace secret also spendable at a registered git host answers only for a repo granted there)",
                caller.model
            );
            return Ok(no_credential());
        }
    };

    // The pair is claimed before the row is written, so two concurrent first
    // serves write one row; a row that fails to land gives the claim back,
    // so the next serve writes it. A failed audit never withholds the
    // credential: auditing must not break what it records.
    let hashed = hosts::hashed_token(&served.password);
    if remember_serve(&agent_id, &hashed, now_ms()) || served.source.minted() {
        let written = try_log_audit(
            &state.pg,
            AuditEntry {
                actor: &caller.model,
                action: credential::CREDENTIAL_ACTION,
                target_type: "agent",
                target_id: Some(&agent_id),
                target_label: Some(&served.name),
                before: None,
                after: Some(audit_after(&served, &host, &hashed)),
            },
        )
        .await;
        if let Err(e) = written {
            forget_serve(&agent_id, &hashed);
            tracing::warn!(
                "[secrets] the git.credential audit row for {} ({}) was not written: {e}",
                caller.model,
                served.name
            );
        }
    }
    tracing::warn!(
        "[secrets] {} spent {} for {host} via git credential helper",
        caller.model,
        served.name
    );
    // A token that goes without part of what the agent was given says so,
    // on git's stderr through the helper: which key, and why (the
    // installation has not accepted it, or GitHub's own words). The default
    // ask, carried whole, says nothing.
    let note = match &served.source {
        Source::GitHost(c) if !c.shortfall.is_empty() => {
            let account = match &c.installation {
                Some(inst) => credential::installation_account(&state.pg, &c.host_id, inst).await,
                None => None,
            };
            credential::shortfall_note(c, account.as_deref())
        }
        _ => None,
    };
    let mut answer = json!({ "username": served.username, "password": served.password });
    if let Some(note) = note {
        answer["note"] = json!(note);
    }
    Ok((NO_STORE, Json(answer)).into_response())
}

/// The one answer every refusal past the identity checks gives the caller.
fn no_credential() -> Response {
    house_error(StatusCode::NOT_FOUND, "no credential for that host")
}

/// Where a served credential came from.
enum Source {
    /// A workspace secret granted to the agent and spendable at the host,
    /// and the registered host and repo it was granted on (None on a host
    /// nobody registered).
    WorkspaceSecret {
        name: String,
        host_id: Option<String>,
        repo: Option<String>,
    },
    /// The registered git host's own login for the granted repo, and what
    /// is known about it.
    GitHost(Box<AgentCredential>),
}

impl Source {
    fn minted(&self) -> bool {
        matches!(self, Source::GitHost(c) if c.minted)
    }
}

/// A credential on its way to git, and what its audit row records.
struct Served {
    username: String,
    password: String,
    /// What the operator log and the audit row call it: the secret's name,
    /// or `<host id>:<repo>`.
    name: String,
    source: Source,
}

/// The `git.credential` row's after-data. Never the token: `hashed_token` is
/// its SHA-256, base64 — exactly how GitHub's audit log records a token, so
/// an operator can follow it there — and nothing a reader could spend.
fn audit_after(served: &Served, host: &str, hashed: &str) -> Value {
    match &served.source {
        Source::WorkspaceSecret {
            name,
            host_id,
            repo,
        } => credential::secret_after(host, name, host_id.as_deref(), repo.as_deref(), hashed),
        Source::GitHost(c) => credential::issued_after(host, c, hashed),
    }
}

/// Record a refusal as a `git.credential_refused` row, at most one per
/// (agent, repo, sentence) in [`REFUSED_TTL_MS`]: git asks on every fetch and
/// push, and one refusal is one line in the trail and in the admin's alert,
/// not one per retry. A row that fails to land gives its claim back.
async fn record_refusal(
    state: &AppState,
    model: &str,
    agent_id: &str,
    host: &str,
    host_id: &str,
    repo: &str,
    refusal: &HostRefusal,
) {
    if !claim_refusal(agent_id, repo, &refusal.message, now_ms()) {
        return;
    }
    let label = format!("{host_id}:{repo}");
    let written = try_log_audit(
        &state.pg,
        AuditEntry {
            actor: model,
            action: credential::REFUSED_ACTION,
            target_type: "agent",
            target_id: Some(agent_id),
            target_label: Some(&label),
            before: None,
            after: Some(credential::refused_after(host, host_id, repo, refusal)),
        },
    )
    .await;
    if let Err(e) = written {
        forget_refusal(agent_id, repo, &refusal.message);
        tracing::warn!(
            "[secrets] the git.credential_refused audit row for {model} ({label}) was not written: {e}"
        );
    }
}

/// How long one refusal is remembered as already recorded.
const REFUSED_TTL_MS: i64 = 10 * 60_000;

type RefusalClaims = Mutex<HashMap<(String, String, String), i64>>;

fn refusal_claims() -> &'static RefusalClaims {
    static CLAIMS: OnceLock<RefusalClaims> = OnceLock::new();
    CLAIMS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Claim the row for `agent_id`'s refusal on `repo` saying `message`. True
/// when none was recorded within the window. Per process, like the serve
/// pairs below.
fn claim_refusal(agent_id: &str, repo: &str, message: &str, now: i64) -> bool {
    let mut claims = refusal_claims().lock().unwrap();
    claims.retain(|_, at| now - *at < REFUSED_TTL_MS);
    let key = (
        agent_id.to_string(),
        repo.to_ascii_lowercase(),
        message.to_string(),
    );
    if claims.contains_key(&key) {
        return false;
    }
    claims.insert(key, now);
    true
}

/// Give back a claim [`claim_refusal`] made, because its row did not land.
fn forget_refusal(agent_id: &str, repo: &str, message: &str) {
    refusal_claims().lock().unwrap().remove(&(
        agent_id.to_string(),
        repo.to_ascii_lowercase(),
        message.to_string(),
    ));
}

/// How long an (agent, token) pair is remembered as already audited. An
/// installation token lives an hour, so a pair is never written twice in
/// its life; a stored secret's token outlives this, and is written again
/// once per window — a trail of who is still spending it.
const SERVED_TTL_MS: i64 = 12 * 60 * 60_000;

type ServedPairs = Mutex<HashMap<(String, String), i64>>;

fn served_pairs() -> &'static ServedPairs {
    static PAIRS: OnceLock<ServedPairs> = OnceLock::new();
    PAIRS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remember that `agent_id` was handed the token whose hash is `hashed`.
/// True when it had not been within the window: the serve that earns a
/// `git.credential` row. Per process, like the token cache it mirrors: a
/// second api process writes its own first row.
fn remember_serve(agent_id: &str, hashed: &str, now: i64) -> bool {
    let mut pairs = served_pairs().lock().unwrap();
    pairs.retain(|_, at| now - *at < SERVED_TTL_MS);
    let key = (agent_id.to_string(), hashed.to_string());
    if pairs.contains_key(&key) {
        return false;
    }
    pairs.insert(key, now);
    true
}

/// Give back a claim [`remember_serve`] made, because its audit row did not
/// land: the next serve of that token to that agent is a first serve again.
fn forget_serve(agent_id: &str, hashed: &str) {
    served_pairs()
        .lock()
        .unwrap()
        .remove(&(agent_id.to_string(), hashed.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_git_hosts::{Level, PermissionSet};

    #[test]
    fn a_pair_is_audited_on_its_first_serve_and_again_only_after_the_window() {
        let agent = format!("agent-{}", uuid::Uuid::new_v4());
        let t0 = 1_000_000;
        assert!(remember_serve(&agent, "h1", t0), "first serve");
        assert!(!remember_serve(&agent, "h1", t0 + 1), "a repeat is not");
        assert!(
            remember_serve(&agent, "h2", t0 + 2),
            "another token is its own first serve"
        );
        assert!(
            remember_serve(&format!("{agent}-other"), "h1", t0 + 3),
            "the same token handed to another agent is that agent's first"
        );
        assert!(
            remember_serve(&agent, "h1", t0 + SERVED_TTL_MS),
            "past the window the pair is written again"
        );
        // A row that did not land gives its claim back.
        let t1 = t0 + SERVED_TTL_MS;
        assert!(!remember_serve(&agent, "h2", t1 + 1), "still claimed");
        forget_serve(&agent, "h2");
        assert!(
            remember_serve(&agent, "h2", t1 + 2),
            "after a failed write the next serve is a first serve"
        );
    }

    fn credential(shortfall: PermissionSet, why: Option<&str>) -> AgentCredential {
        AgentCredential {
            login: talaria_git_hosts::Login {
                username: "x-access-token".into(),
                password: "ghs_secret_value".into(),
            },
            host_id: "github".into(),
            repo: "acme/widgets".into(),
            installation: Some("93001".into()),
            asked: PermissionSet::of(&[
                ("contents", Level::Write),
                ("metadata", Level::Read),
                ("pull_requests", Level::Write),
            ]),
            carried: Some(PermissionSet::of(&[
                ("contents", Level::Write),
                ("metadata", Level::Read),
            ])),
            unheld: PermissionSet::new(),
            omitted: PermissionSet::new(),
            shortfall,
            shortfall_why: why.map(str::to_string),
            minted: true,
        }
    }

    #[test]
    fn the_audit_row_carries_the_hash_and_what_the_token_carries_never_the_token() {
        let served = Served {
            username: "x-access-token".into(),
            password: "ghs_secret_value".into(),
            name: "github:acme/widgets".into(),
            source: Source::GitHost(Box::new(credential(PermissionSet::new(), None))),
        };
        let hashed = hosts::hashed_token(&served.password);
        let after = audit_after(&served, "github.com", &hashed);
        assert_eq!(
            after,
            json!({
                "source": "git_host",
                "host": "github.com",
                "host_id": "github",
                "repo": "acme/widgets",
                "installation": "93001",
                "asked": { "contents": "write", "metadata": "read", "pull_requests": "write" },
                "carried": { "contents": "write", "metadata": "read" },
                "shortfall": {},
                "minted": true,
                "hashed_token": hashed,
            })
        );
        assert!(!after.to_string().contains("ghs_secret_value"));
        assert!(served.source.minted());

        // Organization keys GitHub would not put on the token: the shortfall
        // names them, with GitHub's own words beside it.
        let refused = Served {
            source: Source::GitHost(Box::new(credential(
                PermissionSet::of(&[("organization_secrets", Level::Read)]),
                Some("not on a repository token"),
            ))),
            username: served.username.clone(),
            password: served.password.clone(),
            name: served.name.clone(),
        };
        let after = audit_after(&refused, "github.com", &hashed);
        assert_eq!(
            after["shortfall"],
            json!({ "organization_secrets": "read" })
        );
        assert_eq!(after["shortfallWhy"], "not on a repository token");

        let stored = Served {
            source: Source::WorkspaceSecret {
                name: "deploy-pat".into(),
                host_id: None,
                repo: None,
            },
            name: "deploy-pat".into(),
            ..served
        };
        let after = audit_after(&stored, "gitlab.example.com", "h");
        assert_eq!(after["source"], "workspace_secret");
        assert_eq!(after["secret"], "deploy-pat");
        assert_eq!(after["host_id"], Value::Null);
        assert!(after.get("minted").is_none());
        assert!(!stored.source.minted());
    }

    #[test]
    fn one_refusal_is_one_row_in_its_window() {
        let agent = format!("agent-{}", uuid::Uuid::new_v4());
        let t0 = 5_000_000;
        assert!(claim_refusal(&agent, "acme/widgets", "m1", t0));
        assert!(
            !claim_refusal(&agent, "ACME/Widgets", "m1", t0 + 1),
            "same repo, any case"
        );
        assert!(
            claim_refusal(&agent, "acme/widgets", "m2", t0 + 2),
            "another sentence"
        );
        assert!(
            claim_refusal(&agent, "acme/gadgets", "m1", t0 + 3),
            "another repo"
        );
        assert!(
            claim_refusal(&agent, "acme/widgets", "m1", t0 + REFUSED_TTL_MS),
            "past the window"
        );
        forget_refusal(&agent, "acme/widgets", "m2");
        assert!(claim_refusal(
            &agent,
            "acme/widgets",
            "m2",
            t0 + REFUSED_TTL_MS + 1
        ));
    }

    // ── Live-DB: the door end to end ─────────────────────────────────────────
    //
    // #[ignore]d (house rule: no service-dependent tests in the default run;
    // ci.yml's api job sweeps ignored lib tests in its live-DB steps, against
    // its scratch database). Each test is its own agents, its own git host
    // row (a `.test` origin no other test names) and its own secret, so they
    // run side by side. GitHub is a loopback stub the host row's API origin
    // points at.
    //
    //   DATABASE_URL=postgres://… TALARIA_SECRET_KEY=… \
    //     cargo test -p talaria-routes-integrations git_credential -- --ignored

    mod live {
        use super::super::*;
        use std::sync::Arc;

        use crate::secrets::secrets_git_push_check;
        use talaria_workspace_secrets::{CreateSecret, SecretEntryInput, create_secret_doc};

        async fn state() -> AppState {
            let cfg = talaria_config::Config::from_parts(
                std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the live tests"),
                std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:1".into()),
                std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
                std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
                String::new(),
                "5274".into(),
            )
            .expect("test config");
            let pg = sqlx::PgPool::connect(&cfg.database_url)
                .await
                .expect("DATABASE_URL reachable");
            AppState::new(pg, Arc::new(cfg))
        }

        fn tag() -> String {
            uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
        }

        /// An agent with its own key — the credential a container presents.
        /// Returns (id, model, key).
        async fn agent(state: &AppState, tag: &str, developer: bool) -> (String, String, String) {
            let model = format!("gcl-{tag}-{}", u8::from(developer));
            let (id,): (String,) = sqlx::query_as(
                "insert into agent_defs (slug, department, model, display_name, developer) \
                 values ($1, 'engineering', $1, 'Git Credential Live', $2) returning id::text",
            )
            .bind(&model)
            .bind(developer)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            let sb = state.secretbox().await.expect("secretbox loads");
            let key = talaria_agent_auth::rotate_agent_api_key(&state.pg, &sb, &id)
                .await
                .expect("agent key mints");
            (id, model, key)
        }

        /// A registered GitHub host at `https://<id>.test`, answering on `api`
        /// when given, with `auth` as its connection. Returns (id, authority).
        async fn host(
            state: &AppState,
            tag: &str,
            api: Option<&str>,
            auth: Value,
        ) -> (String, String) {
            let id = format!("gcl-{tag}");
            sqlx::query(
                "insert into git_hosts (id, kind, name, base_url, api_url, auth) \
                 values ($1, 'github', $1, $2, $3, $4)",
            )
            .bind(&id)
            .bind(format!("https://{id}.test"))
            .bind(api)
            .bind(auth)
            .execute(&state.pg)
            .await
            .unwrap();
            let authority = format!("{id}.test");
            (id, authority)
        }

        async fn grant(state: &AppState, agent_id: &str, host_id: &str, repo: &str) {
            sqlx::query(
                "insert into workbench_repos (agent_id, host_id, repo, base_branch) \
                 values ($1::uuid, $2, $3, 'main')",
            )
            .bind(agent_id)
            .bind(host_id)
            .bind(repo)
            .execute(&state.pg)
            .await
            .unwrap();
        }

        async fn cleanup(state: &AppState, agents: &[&str], host_id: &str, secret: Option<&str>) {
            for id in agents {
                sqlx::query(
                    "delete from audit_log where action in ('git.credential', 'git.credential_refused') \
                     and target_id = $1",
                )
                .bind(*id)
                .execute(&state.pg)
                .await
                .unwrap();
                sqlx::query("delete from agent_defs where id = $1::uuid")
                    .bind(*id)
                    .execute(&state.pg)
                    .await
                    .unwrap();
            }
            sqlx::query("delete from git_hosts where id = $1")
                .bind(host_id)
                .execute(&state.pg)
                .await
                .unwrap();
            if let Some(name) = secret {
                talaria_workspace_secrets::delete_secret_doc(&state.pg, name)
                    .await
                    .unwrap();
            }
        }

        async fn answer(res: Result<Response, Response>) -> (StatusCode, Value) {
            let res = res.unwrap_or_else(|r| r);
            let status = res.status();
            let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
                .await
                .unwrap();
            (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
        }

        fn agent_headers(key: &str) -> HeaderMap {
            let mut headers = HeaderMap::new();
            headers.insert("x-api-key", key.parse().unwrap());
            headers
        }

        /// What git's helper sends: the host, https, and the repo's path.
        async fn ask(
            state: &AppState,
            key: &str,
            host: &str,
            path: Option<&str>,
        ) -> (StatusCode, Value) {
            let mut body = json!({ "host": host, "protocol": "https" });
            if let Some(p) = path {
                body["path"] = json!(p);
            }
            answer(
                post(
                    State(state.clone()),
                    agent_headers(key),
                    axum::body::Bytes::from(body.to_string()),
                )
                .await,
            )
            .await
        }

        async fn push_check(
            state: &AppState,
            key: &str,
            host: &str,
            repo: &str,
        ) -> (StatusCode, Value) {
            let body = json!({ "repo": repo, "host": host, "refs": ["refs/heads/agent/x"] });
            answer(
                secrets_git_push_check::post(
                    State(state.clone()),
                    agent_headers(key),
                    axum::body::Bytes::from(body.to_string()),
                )
                .await,
            )
            .await
        }

        /// The `git.credential` rows written for `agent_id`, oldest first.
        async fn credential_rows(state: &AppState, agent_id: &str) -> Vec<Value> {
            sqlx::query_scalar::<_, Option<Value>>(
                "select after from audit_log where action = 'git.credential' and target_id = $1 \
                 order by created_at",
            )
            .bind(agent_id)
            .fetch_all(&state.pg)
            .await
            .unwrap()
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect()
        }

        #[tokio::test]
        #[ignore = "needs a live database (DATABASE_URL)"]
        async fn a_non_developer_agent_gets_no_credential_and_no_push() {
            let state = state().await;
            let tag = tag();
            let (id, model, key) = agent(&state, &tag, false).await;
            let (host_id, authority) = host(&state, &tag, None, json!({})).await;
            grant(&state, &id, &host_id, "acme/widgets").await;

            let (status, body) = ask(&state, &key, &authority, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert_eq!(body["error"], NOT_A_DEVELOPER);
            let (status, body) = push_check(&state, &key, &authority, "acme/widgets").await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert_eq!(
                body["error"], NOT_A_DEVELOPER,
                "the hook shows this sentence"
            );

            // The switch is the gate: on, the same grant answers the push
            // check (a branch that is not the base).
            sqlx::query("update agent_defs set developer = true where model = $1")
                .bind(&model)
                .execute(&state.pg)
                .await
                .unwrap();
            let (status, body) = push_check(&state, &key, &authority, "acme/widgets").await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(credential_rows(&state, &id).await.is_empty());

            cleanup(&state, &[&id], &host_id, None).await;
        }

        #[tokio::test]
        #[ignore = "needs a live database (DATABASE_URL)"]
        async fn a_workspace_secret_on_a_registered_host_answers_only_for_granted_repos() {
            let state = state().await;
            let tag = tag();
            let (id, model, key) = agent(&state, &tag, true).await;
            let (host_id, authority) = host(&state, &tag, None, json!({})).await;
            grant(&state, &id, &host_id, "acme/widgets").await;
            // A PAT pinned for the registered host AND for a host nobody
            // registered.
            let elsewhere = format!("gcl-{tag}-elsewhere.test");
            let name = format!("gcl-{tag}");
            let value = format!("github_pat_{tag}");
            pat(
                &state,
                &name,
                &value,
                &model,
                vec![authority.clone(), elsewhere.clone()],
            )
            .await;
            let spent = |name: String| {
                let pg = state.pg.clone();
                async move {
                    sqlx::query_scalar::<_, bool>(
                        "select last_used_at is not null from workspace_secrets where name = $1",
                    )
                    .bind(name)
                    .fetch_one(&pg)
                    .await
                    .unwrap()
                }
            };

            // A repo the agent was not granted on the registered host: no
            // answer, and the secret was never asked (nothing spent).
            let (status, _) = ask(&state, &key, &authority, Some("acme/other.git")).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _) = ask(&state, &key, &authority, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "no repo named is no answer");
            assert!(
                !spent(name.clone()).await,
                "the workspace store was not asked"
            );
            assert!(credential_rows(&state, &id).await.is_empty());

            // Under no other name: a subdomain of the registered host is no
            // registered authority, and the secret its list covers it by is
            // passed over all the same, for an ungranted repo and a granted
            // one alike. Nor does it answer for the other host on its list,
            // which nobody registered: the value works at the registered
            // host whatever name it was asked under.
            let sub = format!("sub.{authority}");
            let (status, _) = ask(&state, &key, &sub, Some("acme/other.git")).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _) = ask(&state, &key, &sub, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            let (status, _) = ask(&state, &key, &elsewhere, Some("any/repo.git")).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(!spent(name.clone()).await, "nothing was spent");
            assert!(credential_rows(&state, &id).await.is_empty());

            // The granted repo: the secret, which keeps its precedence.
            let (status, body) = ask(&state, &key, &authority, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["password"], value.as_str());
            assert!(spent(name.clone()).await);
            let rows = credential_rows(&state, &id).await;
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0]["source"], "workspace_secret");
            assert_eq!(rows[0]["secret"], name.as_str());
            assert_eq!(rows[0]["host_id"], host_id.as_str());
            assert_eq!(rows[0]["repo"], "acme/widgets");
            assert_eq!(
                rows[0]["hashed_token"],
                hosts::hashed_token(&value).as_str()
            );
            assert!(!rows[0].to_string().contains(&value), "never the token");

            // An unregistered host is unchanged: a secret pinned for it alone
            // answers by host, for any repo.
            let other = format!("gcl-{tag}-elsewhere");
            let other_value = format!("glpat_{tag}");
            pat(
                &state,
                &other,
                &other_value,
                &model,
                vec![elsewhere.clone()],
            )
            .await;
            let (status, body) = ask(&state, &key, &elsewhere, Some("any/repo.git")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["password"], other_value.as_str());
            let rows = credential_rows(&state, &id).await;
            assert_eq!(rows.len(), 2, "{rows:?}");
            assert_eq!(rows[1]["secret"], other.as_str());
            assert_eq!(rows[1]["host_id"], Value::Null);
            // The same tokens again, to the same agent: no more rows.
            let (status, _) = ask(&state, &key, &authority, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::OK);
            let (status, _) = ask(&state, &key, &elsewhere, Some("any/repo.git")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(credential_rows(&state, &id).await.len(), 2);

            talaria_workspace_secrets::delete_secret_doc(&state.pg, &other)
                .await
                .unwrap();
            cleanup(&state, &[&id], &host_id, Some(&name)).await;
        }

        /// A token workspace secret granted to `model`, spendable at `hosts`.
        async fn pat(state: &AppState, name: &str, value: &str, model: &str, hosts: Vec<String>) {
            let sb = state.secretbox().await.expect("secretbox loads");
            create_secret_doc(
                &state.pg,
                &sb,
                &CreateSecret {
                    name: name.to_string(),
                    title: format!("Git credential live {name}"),
                    entries: vec![SecretEntryInput {
                        key: "token".into(),
                        label: "Token".into(),
                        value: value.to_string(),
                    }],
                    kind: None,
                    note: None,
                    created_by: None,
                    expires_at: None,
                    uses: None,
                    grant_to: Some(vec![model.to_string()]),
                    allowed_hosts: Some(hosts),
                    revealable: false,
                    owner_user_id: None,
                    folder_id: None,
                    readers: None,
                },
            )
            .await
            .expect("the secret is created");
        }

        /// What a loopback GitHub saw: method, path, body.
        type Seen = Arc<std::sync::Mutex<Vec<(String, String, String)>>>;

        /// A loopback GitHub answering each request by `route(method, path,
        /// body)`.
        async fn github_with(
            route: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static,
        ) -> (String, Seen) {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
            let seen: Seen = Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = seen.clone();
            let route = Arc::new(route);
            tokio::spawn(async move {
                while let Ok((mut sock, _)) = listener.accept().await {
                    let (log, route) = (log.clone(), route.clone());
                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let mut chunk = [0u8; 4096];
                        let (head, body) = loop {
                            let n = sock.read(&mut chunk).await.unwrap_or(0);
                            if n == 0 {
                                return;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                            let text = String::from_utf8_lossy(&buf).to_string();
                            if let Some(end) = text.find("\r\n\r\n") {
                                let len = text[..end]
                                    .lines()
                                    .find_map(|l| {
                                        let (k, v) = l.split_once(':')?;
                                        k.eq_ignore_ascii_case("content-length")
                                            .then(|| v.trim().parse::<usize>().ok())?
                                    })
                                    .unwrap_or(0);
                                if buf.len() >= end + 4 + len {
                                    break (
                                        text[..end].to_string(),
                                        text[end + 4..end + 4 + len].to_string(),
                                    );
                                }
                            }
                        };
                        let mut words = head.lines().next().unwrap_or_default().split(' ');
                        let method = words.next().unwrap_or_default().to_string();
                        let path = words.next().unwrap_or_default().to_string();
                        log.lock()
                            .unwrap()
                            .push((method.clone(), path.clone(), body.clone()));
                        let (status, answer) = route(&method, &path, &body);
                        let reply = format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                            answer.len()
                        );
                        let _ = sock.write_all(reply.as_bytes()).await;
                        let _ = sock.shutdown().await;
                    });
                }
            });
            (base, seen)
        }

        /// GitHub as the mint meets it: an installation that holds more than
        /// the sandbox asks for, and one token for the one repo.
        async fn github_stub() -> (String, Seen) {
            github_with(|method, path, _| match (method, path) {
                ("GET", "/app/installations/93001") => (
                    200,
                    json!({
                        "id": 93001,
                        "permissions": {
                            "contents": "write",
                            "pull_requests": "write",
                            "workflows": "write",
                            "metadata": "read",
                        },
                    })
                    .to_string(),
                ),
                ("POST", "/app/installations/93001/access_tokens") => (
                    201,
                    json!({
                        "token": "ghs_live_once",
                        "expires_at": "2099-01-01T00:00:00Z",
                        "permissions": { "contents": "write", "metadata": "read" },
                        "repository_selection": "selected",
                        "repositories": [{ "full_name": "acme/widgets" }],
                    })
                    .to_string(),
                ),
                _ => (500, r#"{"message":"unexpected in this test"}"#.to_string()),
            })
            .await
        }

        /// GitHub's 201 for a mint, in the shape GitHub sends it, for
        /// `acme/widgets`, carrying `permissions`.
        fn github_201(token: &str, permissions: Value) -> String {
            let expires = talaria_agent_auth::epoch_ms_to_iso(now_ms() + 3_600_000);
            json!({
                "token": token,
                "expires_at": format!("{}Z", &expires[..19]),
                "permissions": permissions,
                "repository_selection": "selected",
                "repositories": [{
                    "id": 1,
                    "name": "widgets",
                    "full_name": "acme/widgets",
                    "private": true,
                    "owner": { "login": "acme", "id": 1, "type": "Organization" },
                }],
            })
            .to_string()
        }

        /// The App's connection for installation `inst`, sealed with this
        /// state's secretbox.
        async fn app_auth(state: &AppState, inst: &str) -> Value {
            let sb = state.secretbox().await.expect("secretbox loads");
            let pem = include_str!("../../../../tests/it/fixtures/github-jwt-traditional.pem");
            json!({
                "mode": "app",
                "app": {
                    "appId": "12345",
                    "installationIds": [inst],
                    "privateKeyEnc": sb.seal(pem).expect("the key seals"),
                },
            })
        }

        /// The `git.credential_refused` rows written for `agent_id`.
        async fn refused_rows(state: &AppState, agent_id: &str) -> Vec<Value> {
            sqlx::query_scalar::<_, Option<Value>>(
                "select after from audit_log where action = 'git.credential_refused' \
                 and target_id = $1 order by created_at",
            )
            .bind(agent_id)
            .fetch_all(&state.pg)
            .await
            .unwrap()
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect()
        }

        /// A token wider than asked is refused, as it always was: the door
        /// now says so to the agent's git, in the mint's own sentence, and
        /// records it once however often git asks.
        #[tokio::test]
        #[ignore = "needs a live database (DATABASE_URL)"]
        async fn a_token_wider_than_asked_is_a_502_with_the_reason_and_one_refused_row() {
            let state = state().await;
            let tag = tag();
            let (id, _model, key) = agent(&state, &tag, true).await;
            let (base, seen) = github_with(|method, path, _| match (method, path) {
                ("GET", "/app/installations/93101") => (
                    200,
                    json!({
                        "id": 93101,
                        "account": { "login": "acme" },
                        "permissions": {
                            "contents": "write",
                            "pull_requests": "write",
                            "checks": "read",
                            "metadata": "read",
                        },
                    })
                    .to_string(),
                ),
                // Checks never asked for, and on the token.
                ("POST", "/app/installations/93101/access_tokens") => (
                    201,
                    github_201(
                        "ghs_too_wide",
                        json!({ "contents": "write", "metadata": "read", "pull_requests": "write", "checks": "read" }),
                    ),
                ),
                ("DELETE", "/installation/token") => (204, String::new()),
                _ => (500, r#"{"message":"unexpected in this test"}"#.to_string()),
            })
            .await;
            let auth = app_auth(&state, "93101").await;
            let (host_id, authority) = host(&state, &tag, Some(&base), auth).await;
            grant(&state, &id, &host_id, "acme/widgets").await;

            let (status, body) = ask(&state, &key, &authority, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
            let error = body["error"].as_str().expect("the sentence");
            assert!(
                error.contains("it carries checks:read, which was not asked for"),
                "{error}"
            );
            assert!(body.get("password").is_none(), "{body}");
            assert!(!body.to_string().contains("ghs_too_wide"));
            let rows = refused_rows(&state, &id).await;
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0]["kind"], "overbroad");
            assert_eq!(rows[0]["repo"], "acme/widgets");
            assert_eq!(rows[0]["host_id"], host_id.as_str());
            assert_eq!(
                rows[0]["asked"],
                json!({ "contents": "write", "metadata": "read", "pull_requests": "write" })
            );
            assert!(credential_rows(&state, &id).await.is_empty());

            // Git asks again on the next fetch: the same answer, no second row.
            let (status, body) = ask(&state, &key, &authority, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
            assert_eq!(refused_rows(&state, &id).await.len(), 1);
            let revoked = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, p, _)| m == "DELETE" && p == "/installation/token")
                .count();
            assert_eq!(revoked, 2, "each too-wide token was revoked, never served");

            cleanup(&state, &[&id], &host_id, None).await;
        }

        /// An agent given workflows on an installation that has not accepted
        /// it gets the rest, and is told which key is missing and why.
        #[tokio::test]
        #[ignore = "needs a live database (DATABASE_URL)"]
        async fn a_token_short_of_a_given_key_carries_a_note_naming_it() {
            let state = state().await;
            let tag = tag();
            let (id, _model, key) = agent(&state, &tag, true).await;
            let (base, seen) = github_with(|method, path, body| match (method, path) {
                // GitHub refuses what the installation has not accepted.
                ("POST", "/app/installations/93201/access_tokens") if body.contains("workflows") => (
                    422,
                    r#"{"message":"The permissions requested are not granted to this installation.","documentation_url":"https://docs.github.com/rest/apps/apps#create-an-installation-access-token-for-an-app","status":"422"}"#.to_string(),
                ),
                ("POST", "/app/installations/93201/access_tokens") => {
                    let posted: Value = serde_json::from_str(body).unwrap_or_default();
                    (201, github_201("ghs_short", posted["permissions"].clone()))
                }
                _ => (500, r#"{"message":"unexpected in this test"}"#.to_string()),
            })
            .await;
            let auth = app_auth(&state, "93201").await;
            let (host_id, authority) = host(&state, &tag, Some(&base), auth).await;
            grant(&state, &id, &host_id, "acme/widgets").await;
            // The record: the installation, on acme, holds the default.
            sqlx::query(
                "insert into git_host_installations \
                   (host_id, installation_id, account, app_permissions, held, checked_at) \
                 values ($1, 93201, 'acme', $2, $3, now())",
            )
            .bind(&host_id)
            .bind(json!({ "contents": "write", "pull_requests": "write", "workflows": "write", "metadata": "read" }))
            .bind(json!({ "contents": "write", "pull_requests": "write", "metadata": "read" }))
            .execute(&state.pg)
            .await
            .unwrap();
            // An admin gave it workflows besides the default.
            let given = json!({ "contents": "write", "metadata": "read", "pull_requests": "write", "workflows": "write" });
            sqlx::query(
                "insert into agent_github_permissions (agent_id, host_id, permissions) \
                 values ($1::uuid, $2, $3)",
            )
            .bind(&id)
            .bind(&host_id)
            .bind(&given)
            .execute(&state.pg)
            .await
            .unwrap();

            let (status, body) = ask(&state, &key, &authority, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["password"], "ghs_short");
            let note = body["note"].as_str().expect("a note");
            assert!(
                note.contains("not workflows:write")
                    && note.contains("installation on acme has not accepted workflows:write"),
                "{note}"
            );
            assert!(!note.contains('"') && !note.contains('\\'), "{note}");
            let posts: Vec<String> = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, p, _)| m == "POST" && p.ends_with("/access_tokens"))
                .map(|(_, _, b)| b.clone())
                .collect();
            assert_eq!(
                posts.len(),
                1,
                "sized to the record, never asked for workflows: {posts:?}"
            );
            assert!(!posts[0].contains("workflows"), "{}", posts[0]);
            let rows = credential_rows(&state, &id).await;
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0]["asked"], given);
            assert_eq!(rows[0]["shortfall"], json!({ "workflows": "write" }));
            assert_eq!(
                rows[0]["carried"],
                json!({ "contents": "write", "metadata": "read", "pull_requests": "write" })
            );

            sqlx::query("delete from agent_github_permissions where agent_id = $1::uuid")
                .bind(&id)
                .execute(&state.pg)
                .await
                .unwrap();
            cleanup(&state, &[&id], &host_id, None).await;
        }

        #[tokio::test]
        #[ignore = "needs a live database (DATABASE_URL)"]
        async fn a_minted_token_is_audited_once_and_a_cached_serve_is_not() {
            let state = state().await;
            let sb = state.secretbox().await.expect("secretbox loads");
            let tag = tag();
            let (id, _model, key) = agent(&state, &tag, true).await;
            let (base, seen) = github_stub().await;
            let pem = include_str!("../../../../tests/it/fixtures/github-jwt-traditional.pem");
            let auth = json!({
                "mode": "app",
                "app": {
                    "appId": "12345",
                    "installationIds": ["93001"],
                    "privateKeyEnc": sb.seal(pem).expect("the key seals"),
                },
            });
            let (host_id, authority) = host(&state, &tag, Some(&base), auth).await;
            grant(&state, &id, &host_id, "acme/widgets").await;

            let (status, body) = ask(&state, &key, &authority, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["password"], "ghs_live_once");
            let mints: Vec<String> = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, p, _)| m == "POST" && p.ends_with("/access_tokens"))
                .map(|(_, _, b)| b.clone())
                .collect();
            assert_eq!(mints.len(), 1);
            let posted: Value = serde_json::from_str(&mints[0]).unwrap();
            assert_eq!(
                posted["permissions"],
                json!({ "contents": "write", "metadata": "read", "pull_requests": "write" }),
                "an agent nobody set anything for asks for the default, never the installation's workflows"
            );
            let rows = credential_rows(&state, &id).await;
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0]["source"], "git_host");
            assert_eq!(rows[0]["minted"], true);
            assert_eq!(rows[0]["installation"], "93001");
            assert_eq!(
                rows[0]["carried"],
                json!({ "contents": "write", "metadata": "read" })
            );
            assert_eq!(
                rows[0]["hashed_token"],
                hosts::hashed_token("ghs_live_once").as_str()
            );

            // The helper asks again on the next fetch: the cached token, no
            // second mint, no second row.
            let (status, body) = ask(&state, &key, &authority, Some("acme/widgets.git")).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["password"], "ghs_live_once");
            let posts = seen
                .lock()
                .unwrap()
                .iter()
                .filter(|(m, p, _)| m == "POST" && p.ends_with("/access_tokens"))
                .count();
            assert_eq!(posts, 1, "served from the cache");
            assert_eq!(credential_rows(&state, &id).await.len(), 1);

            cleanup(&state, &[&id], &host_id, None).await;
        }
    }
}
