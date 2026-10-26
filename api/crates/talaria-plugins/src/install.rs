// An agent asks for its app to be installed; an admin decides.
//
// THE REQUEST (`file_request`) is the agent's: a repo it was granted, a ref,
// why, and the ticket it is working — the ticket's board is where the
// plugin's maintenance tickets will go, and the ticket's person is who the
// plugin is for. Nothing is fetched or built at request time; the request is
// a row, raised as an `app_install` approval in the census.
//
// THE DECISION (`decide`) is an admin's, in Manage → Apps. Approval runs
// talaria-apps' install — the same credentialed fetch, manifest check, slug
// choice and build an admin's own Install from Git runs — and records the
// plugin. It installs and stops: the app lands DISABLED, and enabling it and
// granting its surfaces stay admin steps. Whatever happens is one line on
// the requesting ticket: installed (and whether it built), the install's
// error, or the rejection and its reason.
//
// Kept data is the one outcome that is not a decision: when an earlier
// install of the slug kept data from another repository, the install refuses
// until the admin says adopt or wipe, and the request stays pending for that
// second answer.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_apps::lifecycle::Credentials;
use talaria_apps::source::ref_ok;
use talaria_apps::{AppHost, InstallError, InstallRequest, KeptDataChoice, KeptDataConflict};
use talaria_audit::{AuditEntry, log_audit};
use talaria_tasks::log_activity;

use crate::{NewPlugin, git_host_of, record_plugin, short_sha};

/// The URL a granted GitHub repository installs from.
pub fn github_url(repo: &str) -> String {
    format!("https://github.com/{repo}")
}

/// `owner/name`, as GitHub spells a repository: one slash, each half made of
/// letters, digits, `.`, `_` and `-`.
pub fn repo_ok(repo: &str) -> bool {
    let mut parts = repo.split('/');
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 100
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    matches!((parts.next(), parts.next(), parts.next()), (Some(o), Some(n), None) if ok(o) && ok(n))
}

/// What an agent asks for.
pub struct InstallAsk<'a> {
    pub agent_id: &'a str,
    pub agent_model: &'a str,
    /// `owner/name` on GitHub — a repo granted to the agent.
    pub repo: &'a str,
    /// A branch, tag or commit; None is the default branch.
    pub git_ref: Option<&'a str>,
    pub why: &'a str,
    pub task_id: &'a str,
    pub board_id: &'a str,
    /// The person the plugin is for.
    pub owner_user_id: Option<&'a str>,
}

/// The person a ticket's work is for: its first human assignee, else the
/// person who created it. None when neither is a user of this workspace.
pub async fn ticket_owner(pg: &PgPool, task_id: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "select u.id::text from tasks t \
         cross join lateral ( \
            select substr(a.value, 6) as id, a.ordinality as rank \
              from jsonb_array_elements_text(t.assignees) with ordinality a(value, ordinality) \
             where a.value like 'user:%' \
            union all \
            select t.created_by, 1000000 \
         ) c \
         join users u on u.id::text = c.id \
         where t.id = $1::uuid \
         order by c.rank limit 1",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
}

/// File the request. Err is the sentence the agent reads.
pub async fn file_request(pg: &PgPool, ask: &InstallAsk<'_>) -> Result<String, String> {
    if !repo_ok(ask.repo) {
        return Err(format!(
            "repo must be owner/name on GitHub — got \"{}\"",
            ask.repo
        ));
    }
    if let Some(r) = ask.git_ref {
        ref_ok(r)?;
    }
    let installed: Option<String> = sqlx::query_scalar(
        "select slug from app_sources where host = 'github.com' and lower(repo) = lower($1) limit 1",
    )
    .bind(ask.repo)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("install lookup failed: {e}"))?;
    if let Some(slug) = installed {
        return Err(format!(
            "{} is already installed here as \"{slug}\" — a merge to its default branch reaches it as an update, not a second install (plugin_status shows it)",
            ask.repo
        ));
    }
    let pending: Option<String> = sqlx::query_scalar(
        "select id::text from app_install_requests \
         where agent_id = $1::uuid and lower(repo) = lower($2) and status in ('pending', 'installing') limit 1",
    )
    .bind(ask.agent_id)
    .bind(ask.repo)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("request lookup failed: {e}"))?;
    if pending.is_some() {
        return Err(
            "you already have a pending install request for this repo — an admin will decide"
                .into(),
        );
    }
    sqlx::query_scalar(
        "insert into app_install_requests \
           (agent_id, agent_model, repo, url, git_ref, why, task_id, board_id, owner_user_id) \
         values ($1::uuid, $2, $3, $4, $5, $6, $7::uuid, $8::uuid, $9::uuid) returning id::text",
    )
    .bind(ask.agent_id)
    .bind(ask.agent_model)
    .bind(ask.repo)
    .bind(github_url(ask.repo))
    .bind(ask.git_ref)
    .bind(ask.why)
    .bind(ask.task_id)
    .bind(ask.board_id)
    .bind(ask.owner_user_id)
    .fetch_one(pg)
    .await
    .map_err(|e| format!("install request write failed: {e}"))
}

/// One request, as the admin queue shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallRequestRow {
    pub id: String,
    pub agent_id: String,
    pub agent_model: String,
    pub agent_name: Option<String>,
    pub repo: String,
    pub url: String,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub why: String,
    pub task_id: Option<String>,
    pub ticket_ref: Option<String>,
    pub board_id: Option<String>,
    pub board_name: Option<String>,
    pub owner_user_id: Option<String>,
    pub owner_name: Option<String>,
    pub status: String,
    pub created_at: String,
}

type RequestRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
);

const REQUEST_SELECT: &str = "select r.id::text, r.agent_id::text, r.agent_model, d.display_name, \
        r.repo, r.url, r.git_ref, r.why, r.task_id::text, \
        case when t.ticket_no is not null then coalesce(tb.ticket_prefix, 'TASK') || '-' || t.ticket_no end, \
        r.board_id::text, b.name, r.owner_user_id::text, coalesce(nullif(u.name, ''), u.email), \
        r.status, (trunc(extract(epoch from r.created_at) * 1000))::bigint \
     from app_install_requests r \
     left join agent_defs d on d.id = r.agent_id \
     left join tasks t on t.id = r.task_id \
     left join boards tb on tb.id = t.board_id \
     left join boards b on b.id = r.board_id \
     left join users u on u.id = r.owner_user_id";

fn request_of(r: RequestRow) -> InstallRequestRow {
    InstallRequestRow {
        id: r.0,
        agent_id: r.1,
        agent_model: r.2,
        agent_name: r.3,
        repo: r.4,
        url: r.5,
        git_ref: r.6,
        why: r.7,
        task_id: r.8,
        ticket_ref: r.9,
        board_id: r.10,
        board_name: r.11,
        owner_user_id: r.12,
        owner_name: r.13,
        status: r.14,
        created_at: epoch_ms_to_iso(r.15),
    }
}

/// The pending queue, oldest first.
pub async fn pending_requests(pg: &PgPool) -> Result<Vec<InstallRequestRow>, sqlx::Error> {
    let rows: Vec<RequestRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{REQUEST_SELECT} where r.status = 'pending' order by r.created_at"
    )))
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(request_of).collect())
}

/// An agent's own requests from the last 30 days, newest first.
pub async fn requests_by_agent(pg: &PgPool, agent_id: &str) -> Result<Vec<Value>, sqlx::Error> {
    let rows: Vec<(
        String,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        i64,
    )> = sqlx::query_as(
        "select repo, git_ref, status, result, decision_note, app_slug, \
                    (trunc(extract(epoch from updated_at) * 1000))::bigint \
             from app_install_requests \
             where agent_id = $1::uuid and created_at > now() - interval '30 days' \
             order by created_at desc limit 10",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(repo, git_ref, status, result, note, slug, at)| {
            json!({
                "repo": repo,
                "ref": git_ref,
                "status": status,
                "result": result,
                "note": note,
                "appSlug": slug,
                "decidedAt": if status == "pending" { Value::Null } else { json!(epoch_ms_to_iso(at)) },
            })
        })
        .collect())
}

/// What the admin decided.
pub enum Decision {
    /// Install. `kept_data` answers an earlier install's kept data, when the
    /// first approval was refused for it.
    Approve {
        kept_data: Option<KeptDataChoice>,
    },
    Reject,
}

/// What a decision did.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Decided {
    /// approved | failed | rejected
    pub status: &'static str,
    /// The line written on the requesting ticket.
    pub line: String,
    pub slug: Option<String>,
    pub sha: Option<String>,
    pub build: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_error: Option<String>,
    /// The app installed but its plugin record could not be written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Why a decision did not land.
#[derive(Debug)]
pub enum DecideError {
    /// No pending request by that id.
    NotFound,
    /// Data an earlier install kept stands in the way: the request stays
    /// pending until the admin answers adopt or wipe.
    KeptData {
        error: String,
        conflict: KeptDataConflict,
    },
    Db(String),
}

/// The words a ref adds to a line: nothing for the default branch.
fn at_ref(git_ref: Option<&str>) -> String {
    git_ref.map(|r| format!(" @ {r}")).unwrap_or_default()
}

fn with_note(line: String, note: Option<&str>) -> String {
    match note {
        Some(n) => format!("{line} — {n}"),
        None => line,
    }
}

/// The build half of the approval line.
fn build_sentence(build: Option<&Value>, build_error: Option<&str>) -> String {
    match (build, build_error) {
        (Some(b), _) if b.get("ok").and_then(Value::as_bool) == Some(true) => "it built".into(),
        (Some(b), _) => format!(
            "its build failed: {}",
            b.get("error")
                .and_then(Value::as_str)
                .unwrap_or("the app host gave no reason")
        ),
        (None, Some(e)) => format!("its build was not run: {e}"),
        (None, None) => "its build was not run".into(),
    }
}

/// The ticket line for each outcome. Pure, so the wording is pinned.
pub fn decision_line(
    outcome: &str,
    repo: &str,
    git_ref: Option<&str>,
    agent_model: &str,
    detail: &str,
    note: Option<&str>,
) -> String {
    let at = at_ref(git_ref);
    let line = match outcome {
        "approved" => format!(
            "approved installing {repo}{at} — {detail}. It is not enabled yet: an admin enables it and grants its surfaces in Manage → Apps"
        ),
        "failed" => format!("approved installing {repo}{at}, but the install failed: {detail}"),
        _ => format!("rejected installing {repo}{at} that {agent_model} asked for"),
    };
    with_note(line, note)
}

/// Decide request `id`. See the module header.
pub async fn decide(
    pg: &PgPool,
    creds: &impl Credentials,
    host: &AppHost,
    id: &str,
    decision: Decision,
    note: Option<&str>,
    actor: &str,
) -> Result<Decided, DecideError> {
    // CLAIM the request: two admins approving at once must not run two
    // installs. A kept-data refusal hands it back to pending.
    let claimed: Option<RequestClaim> = sqlx::query_as(
        "update app_install_requests set status = 'installing', updated_at = now() \
         where id = $1::uuid and status = 'pending' \
         returning agent_id::text, agent_model, repo, url, git_ref, task_id::text, \
                   board_id::text, owner_user_id::text",
    )
    .bind(id)
    .fetch_optional(pg)
    .await
    .map_err(|e| DecideError::Db(format!("request claim failed: {e}")))?;
    let Some((agent_id, agent_model, repo, url, git_ref, task_id, board_id, owner)) = claimed
    else {
        return Err(DecideError::NotFound);
    };

    let finish = |status: &'static str, line: &str, slug: Option<&str>, sha: Option<&str>| {
        let pg = pg.clone();
        let (line, slug, sha) = (
            line.to_string(),
            slug.map(String::from),
            sha.map(String::from),
        );
        let (id, actor, note) = (id.to_string(), actor.to_string(), note.map(String::from));
        async move {
            sqlx::query(
                "update app_install_requests set status = $2, decided_by = $3, decision_note = $4, \
                    result = $5, app_slug = $6, installed_sha = $7, updated_at = now() \
                 where id = $1::uuid",
            )
            .bind(&id)
            .bind(status)
            .bind(&actor)
            .bind(&note)
            .bind(&line)
            .bind(&slug)
            .bind(&sha)
            .execute(&pg)
            .await
            .map_err(|e| DecideError::Db(format!("decision write failed: {e}")))
        }
    };
    let ticket_line = |line: String| {
        let pg = pg.clone();
        let task_id = task_id.clone();
        let actor = actor.to_string();
        async move {
            if let Some(task_id) = task_id
                && let Err(e) = log_activity(&pg, &task_id, &actor, "workbench", &line).await
            {
                tracing::warn!("[plugins] install decision line on {task_id} failed: {e}");
            }
        }
    };

    let kept_data = match decision {
        Decision::Reject => {
            let line = decision_line(
                "rejected",
                &repo,
                git_ref.as_deref(),
                &agent_model,
                "",
                note,
            );
            finish("rejected", &line, None, None).await?;
            ticket_line(line.clone()).await;
            return Ok(Decided {
                status: "rejected",
                line,
                slug: None,
                sha: None,
                build: None,
                build_error: None,
                warning: None,
            });
        }
        Decision::Approve { kept_data } => kept_data,
    };

    let installed = talaria_apps::install_app(
        pg,
        creds,
        Some(host),
        InstallRequest {
            url: &url,
            slug: None,
            git_ref: git_ref.as_deref(),
            secret: None,
            kept_data,
            actor,
        },
    )
    .await;
    match installed {
        Ok(installed) => {
            let host_name = url::Url::parse(&url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_lowercase))
                .unwrap_or_default();
            let warning = record_plugin(
                pg,
                &NewPlugin {
                    slug: &installed.slug,
                    git_host_id: git_host_of(&host_name),
                    maintainer_agent_id: Some(&agent_id),
                    owner_user_id: owner.as_deref(),
                    board_id: board_id.as_deref(),
                    task_id: task_id.as_deref(),
                    created_by: actor,
                },
            )
            .await
            .err()
            .map(|e| format!("installed, but the plugin record could not be written: {e}"));
            let detail = format!(
                "installed as \"{}\" at {}; {}",
                installed.slug,
                short_sha(Some(&installed.sha)),
                build_sentence(installed.build.as_ref(), installed.build_error.as_deref())
            );
            let line = decision_line(
                "approved",
                &repo,
                git_ref.as_deref(),
                &agent_model,
                &detail,
                note,
            );
            finish(
                "approved",
                &line,
                Some(&installed.slug),
                Some(&installed.sha),
            )
            .await?;
            ticket_line(line.clone()).await;
            log_audit(
                pg,
                AuditEntry {
                    actor,
                    action: "app.install",
                    target_type: "app",
                    target_id: Some(&installed.slug),
                    target_label: Some(&url),
                    before: None,
                    after: Some(json!({
                        "sha": installed.sha,
                        "ref": git_ref,
                        "request": id,
                        "maintainer": agent_model,
                        "keptData": installed.kept_data,
                        "wipedData": installed.wiped_data,
                    })),
                },
            )
            .await;
            Ok(Decided {
                status: "approved",
                line,
                slug: Some(installed.slug),
                sha: Some(installed.sha),
                build: installed.build,
                build_error: installed.build_error,
                warning,
            })
        }
        Err(InstallError {
            error,
            kept_data: Some(conflict),
        }) => {
            // Not a decision yet: back to pending for the admin's second
            // answer.
            sqlx::query(
                "update app_install_requests set status = 'pending', updated_at = now() \
                 where id = $1::uuid and status = 'installing'",
            )
            .bind(id)
            .execute(pg)
            .await
            .map_err(|e| DecideError::Db(format!("request release failed: {e}")))?;
            Err(DecideError::KeptData { error, conflict })
        }
        Err(InstallError { error, .. }) => {
            let line = decision_line(
                "failed",
                &repo,
                git_ref.as_deref(),
                &agent_model,
                &error,
                note,
            );
            finish("failed", &line, None, None).await?;
            ticket_line(line.clone()).await;
            log_audit(
                pg,
                AuditEntry {
                    actor,
                    action: "app.install-failed",
                    target_type: "app",
                    target_id: None,
                    target_label: Some(&url),
                    before: None,
                    after: Some(json!({ "error": error, "request": id })),
                },
            )
            .await;
            Ok(Decided {
                status: "failed",
                line,
                slug: None,
                sha: None,
                build: None,
                build_error: None,
                warning: None,
            })
        }
    }
}

/// The columns a claim returns, in order.
type RequestClaim = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repo_is_owner_slash_name() {
        for good in ["acme/crm", "Acme-Labs/talaria-app.notes", "a_b/c_d"] {
            assert!(repo_ok(good), "{good}");
        }
        for bad in [
            "",
            "acme",
            "acme/",
            "/crm",
            "acme/crm/extra",
            "acme/../x",
            "acme/c rm",
            "https://github.com/acme/crm",
        ] {
            assert!(!repo_ok(bad), "{bad}");
        }
        assert_eq!(github_url("acme/crm"), "https://github.com/acme/crm");
    }

    #[test]
    fn each_decision_says_what_happened_on_the_ticket() {
        assert_eq!(
            decision_line(
                "approved",
                "acme/crm",
                None,
                "dev-engineering",
                "installed as \"crm\" at 0123456; it built",
                None
            ),
            "approved installing acme/crm — installed as \"crm\" at 0123456; it built. It is not enabled yet: an admin enables it and grants its surfaces in Manage → Apps"
        );
        assert_eq!(
            decision_line(
                "failed",
                "acme/crm",
                Some("v2"),
                "dev-engineering",
                "talaria.json is refused: missing name",
                None
            ),
            "approved installing acme/crm @ v2, but the install failed: talaria.json is refused: missing name"
        );
        assert_eq!(
            decision_line(
                "rejected",
                "acme/crm",
                None,
                "dev-engineering",
                "",
                Some("not this quarter")
            ),
            "rejected installing acme/crm that dev-engineering asked for — not this quarter"
        );
    }

    #[test]
    fn the_build_sentence_reads_the_hosts_answer() {
        assert_eq!(
            build_sentence(Some(&json!({ "ok": true })), None),
            "it built"
        );
        assert_eq!(
            build_sentence(Some(&json!({ "ok": false, "error": "app.ts: x" })), None),
            "its build failed: app.ts: x"
        );
        assert_eq!(
            build_sentence(None, Some("no app host to build it")),
            "its build was not run: no app host to build it"
        );
    }
}
