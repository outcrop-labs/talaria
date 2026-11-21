// The sandbox's git credential, resolved in one place: the credential door
// (talaria-routes-integrations' `/api/secrets/git-credential`) and an admin's
// "Check token" (talaria-routes-workbench) both walk THIS order, so what the
// check says is what the next push gets.
//
//   1. the agent's Developer switch: off, and its git gets nothing
//   2. the grant (`credential_scope`): on a registered host, a repo this
//      agent holds, before any credential is opened or spent
//   3. a workspace secret granted to the agent and spendable at the host:
//      the door SPENDS it (opened, audited, a one-shot burned); the check
//      PROBES it by name only (`secret_answering_git_at`), never opened and
//      never spent
//   4. the host's own login (`grant_login`): on GitHub a token for this agent
//      alone, carrying its set on the host as far as the installation holds
//      it, served from the cache or minted
//
// A host that will not give a login says why, in its own sentence
// ([`HostRefusal`]): the door hands it to the agent's git (the helper prints
// it) and records a `git.credential_refused` row, and the check shows it. A
// failure on Talaria's own side is never that sentence: a database error, or
// a GitHub App key or PAT that will not unseal, stays an internal error, and
// a Forgejo host whose credential cannot be read is told in a sentence that
// names the host and no secret.
//
// WHAT A TOKEN CARRIED, AND WHY NOT MORE. Each `git.credential` row records
// what was asked, what GitHub put on the token, and what it went without;
// each `git.credential_refused` row records why there was no token. The
// newest of them per repo is what an admin's editor shows as the agent's
// last token ([`last_tokens`]): the one surface that reads what a minted
// token carried, rather than what the installation holds.

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_agent_auth::{agent_is_developer, epoch_ms_to_iso};
use talaria_git_host_types::{Level, Login, PermissionSet};
use talaria_github::MintError;
use talaria_secretbox::SecretBox;

use crate::holdings::{InstallationRow, installation_rows, tracked_installations};
use crate::permissions::installation_for_repo;
use crate::registry::{HostRow, host_rows};
use crate::{AgentCredential, CredentialScope, credential_scope, grant_login, withheld_names};

/// The sentence a non-Developer agent gets, at the door, at the push check
/// beside it, and in an admin's check. It names the switch, because that is
/// what an admin turns.
pub const NOT_A_DEVELOPER: &str = "this agent is not a Developer Agent, so Talaria gives it no git credential and no push — an admin turns on its Developer Agent switch";

/// The audit action of a login the door (or a check) handed out.
pub const CREDENTIAL_ACTION: &str = "git.credential";

/// The audit action of a granted repo the host would give no login for.
pub const REFUSED_ACTION: &str = "git.credential_refused";

// ── Why a login was not given ────────────────────────────────────────────────

/// A host that would not give a login for a granted repo, and why: what the
/// door tells the agent's git and records, and what an admin's check shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRefusal {
    /// The host's sentence (talaria-github's `MintError` display): what an
    /// agent or an admin acts on. It never carries a credential.
    pub message: String,
    /// Which kind of refusal (`MintError::kind`), or `credential` for a
    /// Forgejo host whose own login could not be read.
    pub kind: &'static str,
    /// What the ask named that the installation was known not to hold.
    pub shortfall: PermissionSet,
    /// What the login was asked for.
    pub asked: PermissionSet,
}

impl HostRefusal {
    /// A GitHub mint's failure, for an ask of `asked`.
    pub fn of_mint(e: &MintError, asked: &PermissionSet) -> HostRefusal {
        HostRefusal {
            message: e.to_string(),
            kind: e.kind(),
            shortfall: e.shortfall(),
            asked: asked.clone(),
        }
    }
}

/// Why [`grant_login`] gave no login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantError {
    /// The host would not: its sentence, safe to show the agent and the
    /// admin.
    Host(HostRefusal),
    /// The database failed, or the host's own credential would not unseal:
    /// logged, and a 500 to the caller.
    Internal(String),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantError::Host(r) => f.write_str(&r.message),
            GrantError::Internal(e) => f.write_str(e),
        }
    }
}

impl From<GrantError> for String {
    fn from(e: GrantError) -> String {
        e.to_string()
    }
}

// ── The door's order ─────────────────────────────────────────────────────────

/// How the workspace store is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreRead {
    /// The door: the secret is opened and spent, as git will use it.
    Spend,
    /// An admin's check: the secret is found by name only, never opened and
    /// never spent, so a diagnosis burns no one-shot.
    Probe,
}

/// What git asked for, on whose behalf.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    pub agent_id: &'a str,
    /// The agent's model name: what the workspace store grants by and the
    /// Developer switch is read by.
    pub agent_model: &'a str,
    /// `host[:port]`, as git names it.
    pub authority: &'a str,
    /// The repo path git sent (`owner/name.git`).
    pub path: Option<&'a str>,
}

/// A workspace secret that answers git's ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretAnswer {
    pub name: String,
    /// The login, when the secret was spent. None for a probe: the secret
    /// was found by name and never opened.
    pub login: Option<Login>,
    /// The registered host and repo the ask was granted on. None on a host
    /// nobody registered.
    pub host_id: Option<String>,
    pub repo: Option<String>,
}

/// Where git's ask landed.
#[derive(Debug)]
pub enum Resolution {
    /// The agent's Developer switch is off (or it is disabled).
    NotDeveloper,
    /// A registered host, and the repo is not on this agent's grant list
    /// there, or git named none. Nothing was opened or spent.
    Ungranted {
        host_id: String,
        repo: Option<String>,
    },
    /// A workspace secret answers.
    Secret(SecretAnswer),
    /// The registered host's own login for the granted repo.
    Host(Box<AgentCredential>),
    /// The granted repo's host has no login to give (its connection is not
    /// configured).
    NoLogin { host_id: String, repo: String },
    /// The granted repo's host would not give a login, and said why.
    Refused {
        host_id: String,
        repo: String,
        refusal: HostRefusal,
    },
    /// No registered host is at the authority, and no secret answers there.
    Unanswered,
}

/// Resolve git's ask in the door's order (see the header). Err is the
/// database failing, never a refusal.
pub async fn resolve(
    pg: &PgPool,
    sb: &SecretBox,
    ask: &Ask<'_>,
    read: StoreRead,
) -> Result<Resolution, String> {
    match agent_is_developer(pg, ask.agent_model).await {
        Ok(true) => {}
        Ok(false) => return Ok(Resolution::NotDeveloper),
        Err(e) => return Err(format!("developer check: {e}")),
    }
    let scope = credential_scope(pg, ask.agent_id, ask.authority, ask.path).await?;
    if let CredentialScope::Ungranted { host_id, repo } = scope {
        return Ok(Resolution::Ungranted { host_id, repo });
    }
    let (host_id, repo) = match &scope {
        CredentialScope::Granted { row, rule, .. } => {
            (Some(row.id.clone()), Some(rule.repo.clone()))
        }
        _ => (None, crate::repo_of_path(ask.path.unwrap_or_default())),
    };
    let secret = match read {
        StoreRead::Spend => talaria_workspace_secrets::credential_for_host(
            pg,
            sb,
            ask.agent_model,
            ask.authority,
            scope.withheld(),
        )
        .await?
        .map(|c| {
            (
                c.name,
                Some(Login {
                    username: c.username,
                    password: c.password,
                }),
            )
        }),
        StoreRead::Probe => talaria_workspace_secrets::secret_answering_git_at(
            pg,
            ask.agent_model,
            ask.authority,
            scope.withheld(),
        )
        .await?
        .map(|name| (name, None)),
    };
    if let Some((name, login)) = secret {
        return Ok(Resolution::Secret(SecretAnswer {
            name,
            login,
            host_id,
            repo,
        }));
    }
    let CredentialScope::Granted { row, rule, .. } = scope else {
        return Ok(Resolution::Unanswered);
    };
    match grant_login(pg, sb, &row, &rule.repo, ask.agent_id).await {
        Ok(Some(c)) => Ok(Resolution::Host(Box::new(c))),
        Ok(None) => Ok(Resolution::NoLogin {
            host_id: row.id.clone(),
            repo: rule.repo,
        }),
        Err(GrantError::Host(refusal)) => Ok(Resolution::Refused {
            host_id: row.id.clone(),
            repo: rule.repo,
            refusal,
        }),
        Err(GrantError::Internal(e)) => Err(e),
    }
}

/// The workspace secret that answers git at `row` for `agent_model` before
/// the host's own login does, by name, never opened or spent: the door's own
/// scope, so a secret also spendable at another registered host is passed
/// over here as there. `rows` is every registered host.
pub async fn workspace_secret_at(
    pg: &PgPool,
    agent_model: &str,
    row: &HostRow,
    rows: &[HostRow],
) -> Result<Option<String>, String> {
    let withheld = withheld_names(rows, Some(row.id.as_str()));
    talaria_workspace_secrets::secret_answering_git_at(pg, agent_model, &row.authority(), &withheld)
        .await
}

/// [`workspace_secret_at`], reading the registered hosts itself.
pub async fn workspace_secret_on(
    pg: &PgPool,
    agent_model: &str,
    row: &HostRow,
) -> Result<Option<String>, String> {
    let rows = host_rows(pg).await?;
    workspace_secret_at(pg, agent_model, row, &rows).await
}

// ── What the agent's git is told ─────────────────────────────────────────────

/// A sentence for the credential helper: no double quote, no backslash, no
/// control character, because the helper reads the JSON with sed and prints
/// one line.
pub fn helper_safe(text: &str) -> String {
    text.chars()
        .filter(|c| *c != '"' && *c != '\\')
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// What the agent's git is told beside a token that goes without part of
/// what it was asked for, naming each missing key and why: an installation
/// that has not accepted it (on `account`, when the record names it), or
/// GitHub's own words. None when the token carries all of it. Helper-safe.
pub fn shortfall_note(cred: &AgentCredential, account: Option<&str>) -> Option<String> {
    if cred.shortfall.is_empty() {
        return None;
    }
    let carries = cred
        .carried
        .as_ref()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "what its host gave it".into());
    let mut why: Vec<String> = Vec::new();
    if !cred.unheld.is_empty() {
        let on = account
            .filter(|a| !a.is_empty())
            .map(|a| format!(" on {a}"))
            .unwrap_or_default();
        why.push(format!(
            "the GitHub App's installation{on} has not accepted {} (an admin sees it in Admin → Git hosts)",
            cred.unheld
        ));
    }
    let github = cred.shortfall.without(|k| cred.unheld.contains(k));
    if !github.is_empty() {
        why.push(match &cred.shortfall_why {
            Some(words) => format!("GitHub: {words}"),
            None => format!("GitHub did not put {github} on it"),
        });
    }
    Some(helper_safe(&format!(
        "this token for {} carries {carries} and not {} — {}",
        cred.repo,
        cred.shortfall,
        why.join("; ")
    )))
}

/// The account the installation a token was minted on is installed on, from
/// the record. None when the record does not say.
pub async fn installation_account(
    pg: &PgPool,
    host_id: &str,
    installation: &str,
) -> Option<String> {
    installation_rows(pg, host_id)
        .await
        .ok()?
        .into_iter()
        .find(|r| r.installation_id == installation)
        .map(|r| r.account)
        .filter(|a| !a.is_empty())
}

// ── The audit rows ───────────────────────────────────────────────────────────

/// A `git.credential` row's after-data for a login the host issued. Never
/// the token: `hashed_token` is its SHA-256, base64, exactly how GitHub's
/// audit log records a token, so an operator can follow it there.
pub fn issued_after(host: &str, cred: &AgentCredential, hashed: &str) -> Value {
    let mut after = json!({
        "source": "git_host",
        "host": host,
        "host_id": cred.host_id,
        "repo": cred.repo,
        "installation": cred.installation,
        "asked": cred.asked.to_json(),
        "carried": cred.carried.as_ref().map_or(Value::Null, PermissionSet::to_json),
        "shortfall": cred.shortfall.to_json(),
        "minted": cred.minted,
        "hashed_token": hashed,
    });
    if let Some(why) = &cred.shortfall_why {
        after["shortfallWhy"] = json!(why);
    }
    after
}

/// A `git.credential` row's after-data for a workspace secret that answered.
pub fn secret_after(
    host: &str,
    name: &str,
    host_id: Option<&str>,
    repo: Option<&str>,
    hashed: &str,
) -> Value {
    json!({
        "source": "workspace_secret",
        "secret": name,
        "host": host,
        "host_id": host_id,
        "repo": repo,
        "hashed_token": hashed,
    })
}

/// A `git.credential_refused` row's after-data: the granted repo, the host,
/// which kind of refusal, the host's sentence, and what was asked and known
/// short.
pub fn refused_after(host: &str, host_id: &str, repo: &str, refusal: &HostRefusal) -> Value {
    json!({
        "source": "git_host",
        "host": host,
        "host_id": host_id,
        "repo": repo,
        "kind": refusal.kind,
        "message": refusal.message,
        "asked": refusal.asked.to_json(),
        "shortfall": refusal.shortfall.to_json(),
    })
}

// ── The last token, as the record has it ─────────────────────────────────────

/// The newest `git.credential` or `git.credential_refused` row of one agent
/// for one repo on one host: what its last token carried, or why there was
/// none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastToken {
    pub agent_id: String,
    pub host_id: String,
    pub repo: String,
    /// Epoch ms.
    pub at_ms: i64,
    /// A token was given (`git.credential`), not refused.
    pub ok: bool,
    pub source: Option<String>,
    /// The workspace secret that answered, by name, when one did.
    pub secret: Option<String>,
    /// None on a row written before rows said, and for a workspace secret.
    pub asked: Option<PermissionSet>,
    pub carried: Option<PermissionSet>,
    pub shortfall: Option<PermissionSet>,
    pub shortfall_why: Option<String>,
    /// The refusal's sentence.
    pub error: Option<String>,
    pub minted: Option<bool>,
}

fn set_at(after: &Value, key: &str) -> Option<PermissionSet> {
    after
        .get(key)
        .filter(|v| v.is_object())
        .and_then(|v| PermissionSet::from_json(v).ok())
}

fn text_at(after: &Value, key: &str) -> Option<String> {
    after.get(key).and_then(Value::as_str).map(str::to_string)
}

impl LastToken {
    /// One audit row, read. None for a row that names no host or repo.
    pub fn from_row(agent_id: &str, action: &str, after: &Value, at_ms: i64) -> Option<LastToken> {
        Some(LastToken {
            agent_id: agent_id.to_string(),
            host_id: text_at(after, "host_id")?,
            repo: text_at(after, "repo")?,
            at_ms,
            ok: action == CREDENTIAL_ACTION,
            source: text_at(after, "source"),
            secret: text_at(after, "secret"),
            asked: set_at(after, "asked"),
            carried: set_at(after, "carried"),
            shortfall: set_at(after, "shortfall"),
            shortfall_why: text_at(after, "shortfallWhy"),
            error: text_at(after, "message"),
            minted: after.get("minted").and_then(Value::as_bool),
        })
    }

    /// The editor's wire. `current` says whether the token was asked for
    /// the set the agent holds on the host now (`set`): a token asked for an
    /// older set says nothing about what the next one carries.
    pub fn wire(&self, set: Option<&PermissionSet>) -> Value {
        json!({
            "at": epoch_ms_to_iso(self.at_ms),
            "repo": self.repo,
            "ok": self.ok,
            "source": self.source,
            "secret": self.secret,
            "asked": self.asked.as_ref().map(PermissionSet::to_json),
            "carried": self.carried.as_ref().map(PermissionSet::to_json),
            "shortfall": self.shortfall.as_ref().map(PermissionSet::to_json),
            "shortfallWhy": self.shortfall_why,
            "error": self.error,
            "minted": self.minted,
            "current": matches!((&self.asked, set), (Some(a), Some(s)) if a.canonical() == s.canonical()),
        })
    }
}

/// The newest token row of each of `agent_ids` per (host, repo), newest
/// first. GitHub names ignore case, so a repo is one repo in any case.
pub async fn last_tokens(pg: &PgPool, agent_ids: &[String]) -> Result<Vec<LastToken>, String> {
    if agent_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String, Option<Value>, i64)> = sqlx::query_as(
        "select target_id, action, after, at from ( \
           select distinct on (target_id, after->>'host_id', lower(after->>'repo')) \
                  target_id, action, after, \
                  (trunc(extract(epoch from created_at) * 1000))::bigint as at \
             from audit_log \
            where target_type = 'agent' and target_id = any($1::text[]) \
              and action in ('git.credential', 'git.credential_refused') \
              and after->>'host_id' is not null and after->>'repo' is not null \
            order by target_id, after->>'host_id', lower(after->>'repo'), created_at desc \
         ) newest order by at desc",
    )
    .bind(agent_ids)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("git credential rows read: {e}"))?;
    Ok(rows
        .into_iter()
        .filter_map(|(agent, action, after, at)| {
            LastToken::from_row(&agent, &action, &after.unwrap_or_default(), at)
        })
        .collect())
}

// ── Why a key is not on the token ────────────────────────────────────────────

/// Why one key is not on the agent's token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub key: String,
    pub level: Level,
    /// `notInSet` (the installation holds it; the agent's set does not name
    /// it), `installationLacks` (the set names it; the installation has not
    /// accepted it), `githubRefused` (GitHub would not put it on a token for
    /// one repository), `githubOmitted` (asked for, and left off the token).
    pub why: &'static str,
    pub detail: String,
}

impl Reason {
    pub fn wire(&self) -> Value {
        json!({
            "key": self.key,
            "level": self.level.as_str(),
            "why": self.why,
            "detail": self.detail,
        })
    }
}

/// Every reason a key is not on the agent's token: each key the installation
/// holds (`held`, from the record; metadata aside) that the agent's `set`
/// does not name, and each key the token went without, from the token
/// itself (`issued`) or, when there was none, from the refusal's shortfall.
/// `account` names the installation's account in the details.
pub fn reasons(
    set: &PermissionSet,
    held: Option<&PermissionSet>,
    account: Option<&str>,
    issued: Option<&AgentCredential>,
    refusal: Option<&HostRefusal>,
) -> Vec<Reason> {
    let on = account
        .filter(|a| !a.is_empty())
        .map(|a| format!(" on {a}"))
        .unwrap_or_default();
    let mut out = Vec::new();
    if let Some(held) = held {
        for (key, level) in held.iter() {
            if key == "metadata" || set.contains(key) {
                continue;
            }
            out.push(Reason {
                key: key.to_string(),
                level,
                why: "notInSet",
                detail: format!(
                    "the installation{on} holds {key}:{level}, and this agent's set does not name it — an admin adds it to the agent's set"
                ),
            });
        }
    }
    let lacks = |key: &str, level: Level| Reason {
        key: key.to_string(),
        level,
        why: "installationLacks",
        detail: format!(
            "the GitHub App's installation{on} has not accepted {key}:{level} — the App's owner adds it to the App, then an owner of the account accepts it on GitHub"
        ),
    };
    match (issued, refusal) {
        (Some(c), _) => {
            for (key, level) in c.shortfall.iter() {
                if c.unheld.contains(key) {
                    out.push(lacks(key, level));
                    continue;
                }
                let omitted = c.omitted.contains(key);
                out.push(Reason {
                    key: key.to_string(),
                    level,
                    why: if omitted {
                        "githubOmitted"
                    } else {
                        "githubRefused"
                    },
                    detail: c.shortfall_why.clone().unwrap_or_else(|| {
                        format!("GitHub did not put {key}:{level} on the token")
                    }),
                });
            }
        }
        (None, Some(r)) => {
            for (key, level) in r.shortfall.iter() {
                out.push(lacks(key, level));
            }
        }
        (None, None) => {}
    }
    out
}

/// The installation a token for `repo` on `row` is minted on, as the record
/// has it: the one the token was minted on when that is known, else the one
/// the record routes the repo to.
pub fn minting_installation<'a>(
    row: &HostRow,
    rows: &'a [InstallationRow],
    repo: &str,
    minted_on: Option<&str>,
) -> Option<&'a InstallationRow> {
    match minted_on {
        Some(id) => rows.iter().find(|r| r.installation_id == id),
        None => installation_for_repo(&tracked_installations(row), rows, repo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, Level)]) -> PermissionSet {
        PermissionSet::of(pairs)
    }

    fn cred(
        shortfall: PermissionSet,
        unheld: PermissionSet,
        omitted: PermissionSet,
    ) -> AgentCredential {
        AgentCredential {
            login: Login {
                username: "x-access-token".into(),
                password: "ghs_never_shown".into(),
            },
            host_id: "github".into(),
            repo: "acme/widgets".into(),
            installation: Some("93001".into()),
            asked: set(&[
                ("contents", Level::Write),
                ("metadata", Level::Read),
                ("pull_requests", Level::Write),
                ("workflows", Level::Write),
            ]),
            carried: Some(set(&[
                ("contents", Level::Write),
                ("metadata", Level::Read),
                ("pull_requests", Level::Write),
            ])),
            shortfall_why: (!omitted.is_empty())
                .then(|| format!("GitHub did not put {omitted} on the token")),
            shortfall,
            unheld,
            omitted,
            minted: true,
        }
    }

    #[test]
    fn a_token_short_of_a_key_says_which_and_why_in_a_line_the_helper_can_read() {
        let wf = set(&[("workflows", Level::Write)]);
        let c = cred(wf.clone(), wf.clone(), PermissionSet::new());
        let note = shortfall_note(&c, Some("acme")).expect("a note");
        assert_eq!(
            note,
            "this token for acme/widgets carries contents:write, metadata:read, pull_requests:write and not workflows:write — the GitHub App's installation on acme has not accepted workflows:write (an admin sees it in Admin → Git hosts)"
        );
        assert!(!note.contains("ghs_never_shown"));
        // GitHub's own words, made safe for sed: no quote, no backslash, one line.
        let mut odd = cred(wf.clone(), PermissionSet::new(), wf.clone());
        odd.shortfall_why = Some("left \"workflows\" off\\ the\ntoken".into());
        let note = shortfall_note(&odd, None).unwrap();
        assert!(
            note.contains("GitHub: left workflows off the token"),
            "{note}"
        );
        assert!(!note.contains('"') && !note.contains('\\') && !note.contains('\n'));
        // Nothing short: nothing said.
        let full = cred(
            PermissionSet::new(),
            PermissionSet::new(),
            PermissionSet::new(),
        );
        assert_eq!(shortfall_note(&full, Some("acme")), None);
    }

    #[test]
    fn each_key_off_the_token_has_one_reason() {
        let held = set(&[
            ("actions", Level::Write),
            ("contents", Level::Write),
            ("metadata", Level::Read),
            ("pull_requests", Level::Write),
            ("workflows", Level::Write),
        ]);
        let default = set(&[
            ("contents", Level::Write),
            ("metadata", Level::Read),
            ("pull_requests", Level::Write),
        ]);
        // On the default: what the installation holds beyond it is not in
        // the set, metadata never counted.
        let r = reasons(&default, Some(&held), Some("acme"), None, None);
        let keys: Vec<(&str, &str)> = r.iter().map(|r| (r.key.as_str(), r.why)).collect();
        assert_eq!(keys, [("actions", "notInSet"), ("workflows", "notInSet")]);
        assert!(
            r[1].detail.contains("on acme holds workflows:write"),
            "{}",
            r[1].detail
        );

        // Given workflows, the installation lacks it, and GitHub left
        // issues off.
        let wf = set(&[("workflows", Level::Write)]);
        let issues = set(&[("issues", Level::Write)]);
        let mut c = cred(wf.union(&issues), wf.clone(), issues.clone());
        c.asked = default.union(&wf).union(&issues);
        let r = reasons(&c.asked, Some(&default), None, Some(&c), None);
        let keys: Vec<(&str, &str)> = r.iter().map(|r| (r.key.as_str(), r.why)).collect();
        assert_eq!(
            keys,
            [
                ("issues", "githubOmitted"),
                ("workflows", "installationLacks")
            ]
        );
        assert_eq!(r[0].detail, "GitHub did not put issues:write on the token");

        // An organization key GitHub refused on a repository token.
        let org = set(&[("members", Level::Read)]);
        let mut refused = cred(org.clone(), PermissionSet::new(), PermissionSet::new());
        refused.shortfall_why = Some("Organization permissions cannot be granted".into());
        let r = reasons(&refused.asked, None, None, Some(&refused), None);
        assert_eq!(r[0].why, "githubRefused");
        assert_eq!(r[0].detail, "Organization permissions cannot be granted");

        // No token at all: the refusal's shortfall is what the installation lacks.
        let refusal = HostRefusal {
            message: "the GitHub App's installation holds none of …".into(),
            kind: "nothingHeld",
            shortfall: wf.clone(),
            asked: wf.clone(),
        };
        let r = reasons(&wf, None, None, None, Some(&refusal));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].why, "installationLacks");
    }

    #[test]
    fn the_rows_carry_what_was_asked_and_never_the_token() {
        let wf = set(&[("workflows", Level::Write)]);
        let c = cred(wf.clone(), wf.clone(), PermissionSet::new());
        let after = issued_after("github.com", &c, "hash");
        assert_eq!(after["source"], "git_host");
        assert_eq!(after["asked"]["workflows"], "write");
        assert_eq!(after["shortfall"], json!({ "workflows": "write" }));
        assert!(after.get("shortfallWhy").is_none());
        assert!(!after.to_string().contains("ghs_never_shown"));
        let refusal = HostRefusal {
            message: "GitHub minted a token for acme/widgets wider than Talaria asked for (it carries checks:read, which was not asked for); it was revoked and never used".into(),
            kind: "overbroad",
            shortfall: PermissionSet::new(),
            asked: wf.clone(),
        };
        let after = refused_after("github.com", "github", "acme/widgets", &refusal);
        assert_eq!(after["kind"], "overbroad");
        assert_eq!(after["asked"], json!({ "workflows": "write" }));

        // Read back as the last token: a refusal is not ok and says why; a
        // token asked for the set the agent holds now is current.
        let last = LastToken::from_row("a1", REFUSED_ACTION, &after, 0).unwrap();
        assert!(!last.ok);
        assert!(last.error.as_deref().unwrap().contains("checks:read"));
        let wire = last.wire(Some(&wf));
        assert_eq!(wire["current"], true);
        assert_eq!(wire["at"], "1970-01-01T00:00:00.000Z");
        let issued = LastToken::from_row(
            "a1",
            CREDENTIAL_ACTION,
            &issued_after("github.com", &c, "h"),
            1,
        )
        .unwrap();
        assert!(issued.ok);
        assert_eq!(
            issued.wire(Some(&set(&[("contents", Level::Read)])))["current"],
            false
        );
        // A row written before rows said what was asked is never current.
        let old = json!({ "host_id": "github", "repo": "acme/widgets", "source": "git_host" });
        assert_eq!(
            LastToken::from_row("a1", CREDENTIAL_ACTION, &old, 2)
                .unwrap()
                .wire(Some(&wf))["current"],
            false
        );
        assert!(LastToken::from_row("a1", CREDENTIAL_ACTION, &json!({}), 3).is_none());
        let by_secret = secret_after(
            "github.com",
            "deploy-pat",
            Some("github"),
            Some("acme/widgets"),
            "h",
        );
        let wire = LastToken::from_row("a1", CREDENTIAL_ACTION, &by_secret, 4)
            .unwrap()
            .wire(Some(&wf));
        assert_eq!(wire["source"], "workspace_secret");
        assert_eq!(wire["secret"], "deploy-pat");
        assert_eq!(wire["repo"], "acme/widgets");
        assert_eq!(
            wire["current"], false,
            "a secret's token was not asked for a set"
        );
    }

    #[test]
    fn a_grant_error_reads_as_its_sentence() {
        let e = GrantError::Host(HostRefusal {
            message: "no installation covers acme/x".into(),
            kind: "unrouted",
            shortfall: PermissionSet::new(),
            asked: PermissionSet::new(),
        });
        assert_eq!(String::from(e), "no installation covers acme/x");
        let s: String = GrantError::Internal("db down".into()).into();
        assert_eq!(s, "db down");
    }
}
