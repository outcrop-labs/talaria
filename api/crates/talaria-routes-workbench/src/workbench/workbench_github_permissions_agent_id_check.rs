// /api/workbench/github-permissions/{agentId}/check. "Check token": what the
// agent's git would get for one granted repo NOW, read from the token itself
// rather than from what the installation is recorded to hold.
//
// POST {host, repo} → ADMIN ONLY, like every write to an agent's GitHub
// permissions. It walks the credential door's own order
// (talaria-git-hosts' `credential::resolve`): the Developer switch, the
// grant, a workspace secret granted to the agent and spendable at the host
// (found by NAME only — never opened, never spent, so a one-shot is not
// burned by a diagnosis), then the host's own login. That last step is the
// door's exactly: on GitHub the agent's cached token for the repo is served,
// or the same token a push would get is minted and cached under the agent,
// so the next push uses what this read. It answers
//   {ok, source, secret, installation, asked, carried, shortfall,
//    shortfallWhy, minted, error, reasons, at}
// where `reasons` names, per key, why it is not on the token: `notInSet`
// (the installation holds it; the agent's set does not name it),
// `installationLacks` (the set names it; the installation has not accepted
// it), `githubRefused` (GitHub would not put it on a token for one
// repository), `githubOmitted` (asked for, and left off). The token itself is
// never in the answer. A mint writes the `git.credential` row the door would,
// with the admin as its actor and `check: true`; a refusal writes nothing
// (the trail's refusals are the agent's own git, which the admin alert
// counts).

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::{epoch_ms_to_iso, now_ms};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::credential::{
    self, Ask, Reason, Resolution, SecretAnswer, StoreRead, minting_installation,
};
use talaria_git_hosts::holdings::installation_rows;
use talaria_git_hosts::permissions;
use talaria_git_hosts::{self as hosts, PermissionSet};
use talaria_session::{actor_of, require_admin, secretbox_or_500};
use talaria_state::AppState;

use super::workbench_github_permissions_agent_id::agent_row;

/// The answer's fields, null where they do not apply.
#[derive(Default)]
struct Checked {
    ok: bool,
    source: Option<&'static str>,
    secret: Option<String>,
    installation: Option<String>,
    asked: Option<PermissionSet>,
    carried: Option<PermissionSet>,
    shortfall: Option<PermissionSet>,
    shortfall_why: Option<String>,
    minted: Option<bool>,
    error: Option<String>,
    reasons: Vec<Reason>,
}

impl Checked {
    fn refused(error: String) -> Checked {
        Checked {
            error: Some(error),
            ..Checked::default()
        }
    }

    fn wire(self) -> Value {
        let set = |s: Option<PermissionSet>| s.as_ref().map(PermissionSet::to_json);
        json!({
            "ok": self.ok,
            "source": self.source,
            "secret": self.secret,
            "installation": self.installation,
            "asked": set(self.asked),
            "carried": set(self.carried),
            "shortfall": set(self.shortfall),
            "shortfallWhy": self.shortfall_why,
            "minted": self.minted,
            "error": self.error,
            "reasons": self.reasons.iter().map(Reason::wire).collect::<Vec<_>>(),
            "at": epoch_ms_to_iso(now_ms()),
        })
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let host_id = match string_member(obj, "host", 1, 40) {
        Ok(h) => h,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let repo = match string_member(obj, "repo", 3, 200) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let Some(repo) = hosts::repo_of_path(&repo) else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "repo must be owner/name, as the agent's repo grants name it",
        ));
    };
    if let Some(gate) = talaria_params::uuid_gate_404(&agent_id) {
        return Ok(gate);
    }
    let (agent_label, model) = match agent_row(&state.pg, &agent_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent")),
        Err(e) => {
            return Ok(internal(
                "[workbench/github-permissions] agent read failed",
                e,
            ));
        }
    };
    let row = match hosts::get_host(&state.pg, &host_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "no such git host")),
        Err(e) => {
            return Ok(internal(
                "[workbench/github-permissions] host read failed",
                e,
            ));
        }
    };
    let sb = secretbox_or_500(&state, "[workbench/github-permissions] check failed").await?;
    let set = match permissions::host_permissions(&state.pg, &agent_id, &row.id).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[workbench/github-permissions] read failed", e)),
    };
    let installations = installation_rows(&state.pg, &row.id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("[workbench/github-permissions] {e}");
            Vec::new()
        });
    let authority = row.authority();
    let ask = Ask {
        agent_id: &agent_id,
        agent_model: &model,
        authority: &authority,
        path: Some(&repo),
    };
    let resolution = match credential::resolve(&state.pg, &sb, &ask, StoreRead::Probe).await {
        Ok(r) => r,
        Err(e) => {
            return Ok(internal("[workbench/github-permissions] check failed", e));
        }
    };
    // The installation the token is (or would be) minted on, as the record
    // has it, and what it holds.
    let minted_on = match &resolution {
        Resolution::Host(c) => c.installation.clone(),
        _ => None,
    };
    let on = minting_installation(&row, &installations, &repo, minted_on.as_deref());
    let held = on.and_then(|r| r.held.as_ref());
    let account = on.map(|r| r.account.as_str());
    let checked = match resolution {
        Resolution::NotDeveloper => Checked::refused(credential::NOT_A_DEVELOPER.to_string()),
        Resolution::Ungranted { .. } => Checked::refused(format!(
            "{repo} is not one of this agent's repos on {}, so its git gets no credential there — grant the repo first",
            row.name
        )),
        Resolution::Secret(SecretAnswer { name, .. }) => Checked {
            ok: true,
            source: Some("workspace_secret"),
            secret: Some(name),
            ..Checked::default()
        },
        Resolution::Host(c) => {
            if c.minted {
                let hashed = hosts::hashed_token(&c.login.password);
                let mut after = credential::issued_after(&authority, &c, &hashed);
                after["check"] = json!(true);
                let label = format!("{}:{}", c.host_id, c.repo);
                log_audit(
                    &state.pg,
                    AuditEntry {
                        actor: &actor_of(&user),
                        action: credential::CREDENTIAL_ACTION,
                        target_type: "agent",
                        target_id: Some(&agent_id),
                        target_label: Some(&label),
                        before: None,
                        after: Some(after),
                    },
                )
                .await;
            }
            Checked {
                ok: true,
                source: Some("git_host"),
                installation: c.installation.clone(),
                reasons: credential::reasons(&c.asked, held, account, Some(c.as_ref()), None),
                asked: Some(c.asked.clone()),
                carried: c.carried.clone(),
                shortfall: Some(c.shortfall.clone()),
                shortfall_why: c.shortfall_why.clone(),
                minted: Some(c.minted),
                ..Checked::default()
            }
        }
        Resolution::NoLogin { .. } => Checked::refused(format!(
            "{} has no login to give for {repo}: its connection is not finished (Admin → Git hosts)",
            row.name
        )),
        Resolution::Refused { refusal, .. } => Checked {
            reasons: credential::reasons(&set, held, account, None, Some(&refusal)),
            installation: on.map(|r| r.installation_id.clone()),
            asked: Some(refusal.asked.clone()),
            shortfall: Some(refusal.shortfall.clone()),
            error: Some(refusal.message),
            ..Checked::default()
        },
        Resolution::Unanswered => {
            Checked::refused(format!("nothing answers git at {authority} for this agent"))
        }
    };
    tracing::info!(
        "[workbench/github-permissions] {} checked {agent_label}'s git token for {repo} on {}: {}",
        actor_of(&user),
        row.id,
        match (&checked.error, &checked.carried) {
            (Some(e), _) => format!("none ({e})"),
            (None, Some(c)) => format!("carries {c}"),
            (None, None) => format!("{} answers", checked.source.unwrap_or("nothing")),
        }
    );
    Ok(Json(checked.wire()).into_response())
}

#[cfg(test)]
mod tests {
    // ── Live-DB: the check end to end ────────────────────────────────────────
    //
    // #[ignore]d (house rule: no service-dependent tests in the default run;
    // ci.yml's api job sweeps ignored lib tests in its live-DB steps, against
    // its scratch database and Redis). GitHub is a loopback stub the host
    // row's API origin points at.
    //
    //   DATABASE_URL=postgres://… REDIS_URL=… TALARIA_SECRET_KEY=… \
    //     cargo test -p talaria-routes-workbench github_permissions -- --ignored

    mod live {
        use super::super::*;
        use crate::workbench::live_support::{
            agent, answer, bodies, cleanup, github, github_host, person, state, tag,
        };

        async fn check_as(
            state: &AppState,
            headers: &HeaderMap,
            agent: &str,
            body: Value,
        ) -> (StatusCode, Value) {
            answer(
                post(
                    State(state.clone()),
                    headers.clone(),
                    Path(agent.to_string()),
                    Bytes::from(body.to_string()),
                )
                .await,
            )
            .await
        }

        fn reason<'a>(body: &'a Value, key: &str) -> Option<&'a Value> {
            body["reasons"]
                .as_array()
                .and_then(|r| r.iter().find(|r| r["key"] == key))
        }

        #[tokio::test]
        #[ignore = "needs a live database and Redis (DATABASE_URL, REDIS_URL)"]
        async fn check_token_reads_what_the_agents_token_carries_and_why_not_more() {
            let state = state().await;
            let t = tag();
            let (admin, as_admin) = person(&state, &t, "admin", "admin").await;
            let (manager, as_manager) = person(&state, &t, "manager", "member").await;
            let a = agent(&state, &t, &manager).await;
            let (base, log) = github("95101").await;
            let h = github_host(&state, &t, &base, "95101").await;
            talaria_git_hosts::add_grant(&state.pg, &a, &h, "acme/widgets")
                .await
                .unwrap();
            let check = json!({ "host": h, "repo": "acme/widgets" });

            // Admins only: the agent's manager may not.
            let (status, _) = check_as(&state, &as_manager, &a, check.clone()).await;
            assert_eq!(status, StatusCode::FORBIDDEN);

            // On the default: the token carries the default, and workflows,
            // which the installation holds, is named as not in the set.
            let (status, body) = check_as(&state, &as_admin, &a, check.clone()).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["ok"], true, "{body}");
            assert_eq!(body["source"], "git_host");
            assert_eq!(body["installation"], "95101");
            assert_eq!(body["minted"], true);
            assert_eq!(
                body["carried"],
                json!({ "contents": "write", "metadata": "read", "pull_requests": "write" })
            );
            assert_eq!(body["shortfall"], json!({}));
            let wf = reason(&body, "workflows").expect("workflows is named");
            assert_eq!(wf["why"], "notInSet", "{body}");
            assert_eq!(wf["level"], "write");
            assert!(reason(&body, "metadata").is_none(), "{body}");
            assert!(
                !body.to_string().contains("tok-"),
                "never the token: {body}"
            );
            let rows: Vec<(String, Value)> = sqlx::query_as(
                "select actor, after from audit_log where action = 'git.credential' \
                 and target_id = $1 order by created_at",
            )
            .bind(&a)
            .fetch_all(&state.pg)
            .await
            .unwrap();
            assert_eq!(rows.len(), 1, "{rows:?}");
            assert_eq!(rows[0].1["check"], true);
            assert_eq!(rows[0].1["minted"], true);
            assert_ne!(rows[0].0, "talaria", "the admin is the actor");

            // The editor shows that token as the repo's last, current for the
            // set the agent holds now, and the admin may check.
            let (_, editor) = answer(get_editor(&state, &as_admin, &a).await).await;
            assert_eq!(editor["canCheck"], true);
            let last = &editor["hosts"][0]["repos"][0]["lastToken"];
            assert_eq!(last["ok"], true, "{editor}");
            assert_eq!(last["current"], true, "{editor}");
            assert_eq!(last["carried"], body["carried"]);
            assert_eq!(editor["hosts"][0]["workspaceSecret"], Value::Null);

            // A second check serves the cached token: no second mint, no row.
            let (_, again) = check_as(&state, &as_admin, &a, check.clone()).await;
            assert_eq!(again["minted"], false);
            assert_eq!(
                bodies(&log, "POST", "/app/installations/95101/access_tokens")
                    .into_iter()
                    .filter(|b| !b.is_empty())
                    .count(),
                1
            );

            // The admin grants workflows: the next check's token carries it.
            let given = json!({ "contents": "write", "metadata": "read", "pull_requests": "write", "workflows": "write" });
            let (status, _) = answer(
                crate::workbench::workbench_github_permissions_agent_id::put(
                    State(state.clone()),
                    as_admin.clone(),
                    Path(a.clone()),
                    Bytes::from(json!({ "host": h, "from": null, "to": given }).to_string()),
                )
                .await,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let (_, granted) = check_as(&state, &as_admin, &a, check.clone()).await;
            assert_eq!(granted["ok"], true, "{granted}");
            assert_eq!(granted["minted"], true);
            assert_eq!(granted["carried"]["workflows"], "write", "{granted}");
            assert_eq!(granted["asked"], given);
            assert!(reason(&granted, "workflows").is_none(), "{granted}");
            // The editor's last token from before the grant was asked for the
            // old set; the newest is the one just minted, and current.
            let (_, editor) = answer(get_editor(&state, &as_admin, &a).await).await;
            assert_eq!(
                editor["hosts"][0]["repos"][0]["lastToken"]["carried"]["workflows"],
                "write"
            );

            // A workspace secret answers git at the host first: the check
            // says so by name, and the secret is neither opened nor spent.
            let model: String =
                sqlx::query_scalar("select model from agent_defs where id = $1::uuid")
                    .bind(&a)
                    .fetch_one(&state.pg)
                    .await
                    .unwrap();
            let name = format!("wb-check-{t}");
            let sb = state.secretbox().await.unwrap();
            let row = talaria_git_hosts::get_host(&state.pg, &h)
                .await
                .unwrap()
                .unwrap();
            // A one-shot token secret granted to the agent and spendable at
            // the host, as the Secrets page writes it: sealed, one entry.
            sqlx::query(
                "with s as ( \
                   insert into workspace_secrets (name, title, uses_remaining, allowed_hosts) \
                   values ($1, $2, 1, $3) returning id \
                 ), e as ( \
                   insert into workspace_secret_entries (secret_id, key, label, value_cipher) \
                   select id, 'token', 'Token', $4 from s \
                 ) \
                 insert into workspace_secret_grants (secret_id, agent_model) select id, $5 from s",
            )
            .bind(&name)
            .bind(format!("Check live {t}"))
            .bind(vec![row.authority()])
            .bind(sb.seal(&format!("github_pat_{t}")).expect("seals"))
            .bind(&model)
            .execute(&state.pg)
            .await
            .expect("the secret");
            let spent = || {
                let (pg, name) = (state.pg.clone(), name.clone());
                async move {
                    sqlx::query_as::<_, (Option<i32>, bool)>(
                        "select uses_remaining, last_used_at is not null from workspace_secrets where name = $1",
                    )
                    .bind(name)
                    .fetch_one(&pg)
                    .await
                    .unwrap()
                }
            };
            let before = spent().await;
            let (_, by_secret) = check_as(&state, &as_admin, &a, check.clone()).await;
            assert_eq!(by_secret["ok"], true, "{by_secret}");
            assert_eq!(by_secret["source"], "workspace_secret");
            assert_eq!(by_secret["secret"], name.as_str());
            assert_eq!(by_secret["carried"], Value::Null);
            assert!(
                !by_secret.to_string().contains("github_pat_"),
                "{by_secret}"
            );
            assert_eq!(
                spent().await,
                before,
                "probed by name: uses_remaining unchanged, never used"
            );
            assert_eq!(before.0, Some(1));
            let (_, editor) = answer(get_editor(&state, &as_admin, &a).await).await;
            assert_eq!(editor["hosts"][0]["workspaceSecret"], name.as_str());

            // An ungranted repo, and a repo that is no repo.
            let (_, other) = check_as(
                &state,
                &as_admin,
                &a,
                json!({ "host": h, "repo": "acme/other" }),
            )
            .await;
            assert_eq!(other["ok"], false);
            assert!(
                other["error"]
                    .as_str()
                    .unwrap()
                    .contains("not one of this agent's repos"),
                "{other}"
            );
            let (status, _) =
                check_as(&state, &as_admin, &a, json!({ "host": h, "repo": "nope" })).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);

            sqlx::query("delete from workspace_secrets where name = $1")
                .bind(&name)
                .execute(&state.pg)
                .await
                .unwrap();
            cleanup(&state, &[&a], &[&admin, &manager], &[&h]).await;
        }

        async fn get_editor(
            state: &AppState,
            headers: &HeaderMap,
            agent: &str,
        ) -> Result<Response, Response> {
            crate::workbench::workbench_github_permissions_agent_id::get(
                State(state.clone()),
                headers.clone(),
                Path(agent.to_string()),
            )
            .await
        }
    }
}
