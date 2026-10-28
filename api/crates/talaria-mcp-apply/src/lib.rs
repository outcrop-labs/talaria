// Applying MCP registry changes to RUNNING agents. What a running Hermes
// takes on its own, and what this crate does about the rest:
//
// · A server GRANTED or REVOKED is a name in the agent's `mcp_servers`. The
//   render rewrites config.yaml in place and Hermes reconciles the live
//   servers against it by name about once a minute, so those changes APPLY
//   (fleet-reconcile's `apply_agent_change`: render, recreate only a
//   container the render moved — an MCP edit moves none).
// · A tool list that changes UNDER a name the agent already holds — a tool
//   subset widened, a team or user grant, an app's catalog — reaches it
//   through the gateway: [`bump_tool_lists`] counts the change for that
//   server, and the gateway answers the agent's next keepalive on that
//   server (a `ping`, or
//   the `tools/list` Hermes keeps alive with once an upstream did not answer
//   ping) with a `notifications/tools/list_changed` first (routes-fleet's
//   mcp_gw_server), which Hermes answers by listing the tools again.
// · A `timeout` or an auth mode changed under a name still ROLLS: the
//   reconcile does not reconnect a server whose entry changed, and a session
//   minted under the old credentials is the old session. A roll recreates a
//   RUNNING carrier only: one a person stopped stays stopped, and its next
//   start boots Hermes on the rewritten config (fleet-reconcile's
//   `roll_running_agent` — no roll starts a stopped agent).
//
// Both kinds run sequentially off one deduped queue — a burst of registry
// edits becomes one render and at most one roll per department, and a
// department queued for both is rolled once.

use serde_json::json;
use sqlx::PgPool;
use std::sync::{LazyLock, Mutex};

use talaria_audit::{AuditEntry, log_audit};
use talaria_secretbox::SecretBox;

use futures_util::future::BoxFuture;
use std::sync::{Arc, OnceLock};

/// Wired from the api binary so this crate does not depend on fleet reconcile:
/// fleet-reconcile's `roll_running_agent` — recreate a running agent, leave a
/// stopped one stopped.
pub static ROLL_AGENT: OnceLock<
    Arc<
        dyn Fn(
                sqlx::PgPool,
                SecretBox,
                String,
            ) -> BoxFuture<'static, Result<Option<String>, String>>
            + Send
            + Sync,
    >,
> = OnceLock::new();

/// One verdict per department, in the order asked: what
/// [`APPLY_AGENTS`] answers.
pub type ApplyVerdicts = Vec<(String, Result<Option<String>, String>)>;

/// Wired from the api binary: fleet-reconcile's `apply_agent_changes` — one
/// render, then a roll only for a container the render moved.
pub static APPLY_AGENTS: OnceLock<
    Arc<
        dyn Fn(sqlx::PgPool, SecretBox, Vec<String>) -> BoxFuture<'static, ApplyVerdicts>
            + Send
            + Sync,
    >,
> = OnceLock::new();

/// What a queued department is owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Render, and recreate only if the container moved.
    Apply,
    /// Recreate a running agent regardless of its fit — the change is one
    /// Hermes takes only from a fresh start. A stopped agent is left as it
    /// is; its next start is that fresh start.
    Roll,
}

/// The process-local queue: deduped departments with what each is owed, plus
/// a running flag the pump clears when it drains.
static QUEUE: LazyLock<Mutex<(Vec<(String, Mode)>, bool)>> =
    LazyLock::new(|| Mutex::new((Vec::new(), false)));

/// Merge a request into the queue: a department already queued keeps its
/// place, and a roll supersedes an apply (a roll renders too).
fn merge_into(queue: &mut Vec<(String, Mode)>, departments: &[String], mode: Mode) {
    for d in departments {
        match queue.iter_mut().find(|(q, _)| q == d) {
            Some(entry) => {
                if mode == Mode::Roll {
                    entry.1 = Mode::Roll;
                }
            }
            None => queue.push((d.clone(), mode)),
        }
    }
}

/// The one pump. Lazily armed so a test environment with no runtime still
/// links; in the process it's the tokio handle the routes spawn under. Each
/// turn takes everything queued: the applies share one render, then the
/// rolls run one at a time.
fn enqueue(departments: &[String], mode: Mode, pg: &PgPool, sb: &SecretBox) {
    {
        let mut q = QUEUE.lock().unwrap_or_else(|p| p.into_inner());
        merge_into(&mut q.0, departments, mode);
        if q.1 {
            return;
        }
        q.1 = true;
    }
    let pg = pg.clone();
    let sb = sb.clone();
    tokio::spawn(async move {
        loop {
            let batch: Vec<(String, Mode)> = {
                let mut q = QUEUE.lock().unwrap_or_else(|p| p.into_inner());
                if q.0.is_empty() {
                    q.1 = false;
                    return;
                }
                std::mem::take(&mut q.0)
            };
            let (rolls, applies): (Vec<_>, Vec<_>) =
                batch.into_iter().partition(|(_, m)| *m == Mode::Roll);
            let applies: Vec<String> = applies.into_iter().map(|(d, _)| d).collect();
            let mut verdicts: ApplyVerdicts = Vec::new();
            if !applies.is_empty()
                && let Some(f) = APPLY_AGENTS.get()
            {
                verdicts.extend(f(pg.clone(), sb.clone(), applies).await);
            }
            for (department, _) in rolls {
                let rolled = match ROLL_AGENT.get() {
                    Some(f) => f(pg.clone(), sb.clone(), department.clone()).await,
                    None => Ok(None),
                };
                verdicts.push((department, rolled));
            }
            for (department, verdict) in verdicts {
                // `{ok:false}` AND a thrown step both land here: the verdict
                // is a sentence for the operator's audit trail, never a
                // caller's problem — the mutation that triggered it is
                // already committed.
                let error = match verdict {
                    Ok(None) => continue,
                    Ok(Some(error)) => error,
                    Err(thrown) => thrown,
                };
                log_audit(
                    &pg,
                    AuditEntry {
                        actor: "talaria",
                        action: "mcp.roll_failed",
                        target_type: "department",
                        target_id: Some(&department),
                        target_label: None,
                        before: None,
                        after: Some(json!({ "error": error })),
                    },
                )
                .await;
            }
        }
    });
}

/// The departments whose managed agents carry this server right now — every
/// grant `servers_for_agent` renders from: all-agents, an assignment row, or
/// an allowed team grant for a team the agent is on. A team-granted agent
/// left out here would miss the apply or roll its server's change asks for
/// (most visibly: a timeout or auth-mode edit, which only a recreate
/// delivers, and the agent stays on its old session until something else
/// rolls it).
pub async fn carriers_for_server(pg: &PgPool, server_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let all_agents: Option<(bool,)> =
        sqlx::query_as("select all_agents from mcp_servers where id::text = $1")
            .bind(server_id)
            .fetch_optional(pg)
            .await?;
    let rows: Vec<(String,)> = if all_agents.is_some_and(|(a,)| a) {
        sqlx::query_as("select distinct department from agent_defs where managed and enabled")
            .fetch_all(pg)
            .await?
    } else {
        sqlx::query_as(
            "select distinct d.department from agent_defs d \
             where d.managed and d.enabled and ( \
               exists (select 1 from mcp_server_agents a \
                       where a.server_id::text = $1 and a.agent_model = d.model) \
               or exists (select 1 from mcp_team_access t \
                          join team_agents ta on ta.team_id = t.team_id \
                          where t.server_id::text = $1 and t.allowed and ta.agent_model = d.model))",
        )
        .bind(server_id)
        .fetch_all(pg)
        .await?
    };
    Ok(rows.into_iter().map(|(d,)| d).collect())
}

async fn departments_for_user(pg: &PgPool, user_id: &str) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select department from agent_defs where owner_user_id = $1::uuid and managed and enabled",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    rows.into_iter().map(|(d,)| d).collect()
}

async fn departments_for_model(pg: &PgPool, model: &str) -> Vec<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select department from agent_defs where model = $1 and managed and enabled",
    )
    .bind(model)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    rows.into_iter().map(|(d,)| d).collect()
}

// ── Apply: render, recreate only what moved ─────────────────────────────────

/// Queue an apply for an explicit department list (e.g. captured pre-delete).
pub fn enqueue_applies(departments: &[String], pg: &PgPool, sb: &SecretBox) {
    enqueue(departments, Mode::Apply, pg, sb);
}

/// Apply a change to every managed agent that carries this server.
pub async fn apply_agents_for_server(pg: &PgPool, sb: &SecretBox, server_id: &str) {
    if let Ok(departments) = carriers_for_server(pg, server_id).await {
        enqueue(&departments, Mode::Apply, pg, sb);
    }
}

/// Apply a change to one user's personal assistant.
pub async fn apply_agent_for_user(pg: &PgPool, sb: &SecretBox, user_id: &str) {
    let departments = departments_for_user(pg, user_id).await;
    enqueue(&departments, Mode::Apply, pg, sb);
}

/// Apply a change to one specific agent model's department.
pub async fn apply_agent_for_model(pg: &PgPool, sb: &SecretBox, model: &str) {
    let departments = departments_for_model(pg, model).await;
    enqueue(&departments, Mode::Apply, pg, sb);
}

// ── Roll: recreate regardless ───────────────────────────────────────────────

/// Roll every running managed agent that carries this server — for an edit
/// Hermes takes only from a reconnect (a timeout, an auth mode). A stopped
/// carrier stays stopped and reads the edit when it next starts.
pub async fn roll_agents_for_server(pg: &PgPool, sb: &SecretBox, server_id: &str) {
    if let Ok(departments) = carriers_for_server(pg, server_id).await {
        enqueue(&departments, Mode::Roll, pg, sb);
    }
}

/// Roll one specific agent model's department — the Developer switch and a
/// key rotation, both of which change the container's own env. Only a
/// running agent is recreated; a stopped one's next start recreates it, since
/// its service moved.
pub async fn roll_agent_for_model(pg: &PgPool, sb: &SecretBox, model: &str) {
    let departments = departments_for_model(pg, model).await;
    enqueue(&departments, Mode::Roll, pg, sb);
}

// ── Tool lists that change under a name ─────────────────────────────────────

/// The `app_settings` key counting changes to what agents' tool lists for ONE
/// server should hold. The gateway compares a server's count with the count
/// each agent last saw on that server, and tells an agent behind it to list
/// that server again. One count per server: a change to one server re-lists
/// that server alone, for the agents that hold it — never every server every
/// agent holds (the builtin toolkit's 82 tools among them), which is what one
/// count for everything did.
pub fn tool_list_count_key(server_name: &str) -> String {
    format!("{TOOL_LIST_COUNT_PREFIX}{server_name}")
}

/// The prefix of [`tool_list_count_key`] — the TS app gateway's twin
/// (`mcp-tool-list-notice.ts`) spells the same keys.
pub const TOOL_LIST_COUNT_PREFIX: &str = "mcp.tool_list_count:";

/// Count a change that may alter a tool list an agent already holds under a
/// server name it keeps — a subset widened or narrowed, a team or user
/// grant, an app's new catalog, a refreshed catalog, a connected account —
/// once for each server named. Over-counting is harmless: an agent told to
/// list again lists the same tools. Call it after the change commits, so the
/// list it prompts reads the new state. Best effort; a failure is logged, and
/// the agents keep the lists they have until their next reconnect.
pub async fn bump_tool_lists(pg: &PgPool, server_names: &[String]) {
    if server_names.is_empty() {
        return;
    }
    let bumped = sqlx::query(
        "insert into app_settings (key, value) \
         select $1::text || name, '1'::jsonb from unnest($2::text[]) as name \
         on conflict (key) do update \
           set value = to_jsonb(coalesce((app_settings.value #>> '{}')::bigint, 0) + 1), \
               updated_at = now()",
    )
    .bind(TOOL_LIST_COUNT_PREFIX)
    .bind(dedup(server_names))
    .execute(pg)
    .await;
    if let Err(e) = bumped {
        tracing::warn!(
            "[mcp] tool-list change for {} not counted ({e}) — their agents keep their lists until they reconnect",
            server_names.join(", ")
        );
    }
}

/// [`bump_tool_lists`] for registry rows named by id — what most writers hold.
/// An id with no row (already deleted) counts nothing: no agent can hold it.
pub async fn bump_tool_lists_for_ids(pg: &PgPool, server_ids: &[String]) {
    if server_ids.is_empty() {
        return;
    }
    let names: Result<Vec<(String,)>, _> =
        sqlx::query_as("select name from mcp_servers where id::text = any($1)")
            .bind(server_ids)
            .fetch_all(pg)
            .await;
    match names {
        Ok(rows) => {
            let names: Vec<String> = rows.into_iter().map(|(n,)| n).collect();
            bump_tool_lists(pg, &names).await;
        }
        Err(e) => tracing::warn!(
            "[mcp] tool-list change not counted — server names unreadable ({e}); their agents keep their lists until they reconnect"
        ),
    }
}

/// The names in their first order, each once — one row per server per bump.
fn dedup(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for n in names {
        if !out.contains(n) {
            out.push(n.clone());
        }
    }
    out
}

/// One server's current count ([`bump_tool_lists`]); `None` when it cannot be
/// read, 0 before its first change.
pub async fn tool_list_count(pg: &PgPool, server_name: &str) -> Option<i64> {
    sqlx::query_as::<_, (Option<i64>,)>(
        "select (value #>> '{}')::bigint from app_settings where key = $1",
    )
    .bind(tool_list_count_key(server_name))
    .fetch_optional(pg)
    .await
    .ok()
    .map(|row| row.and_then(|(n,)| n).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(items: &[(&str, Mode)]) -> Vec<(String, Mode)> {
        items.iter().map(|(d, m)| (d.to_string(), *m)).collect()
    }

    #[test]
    fn a_burst_dedupes_and_a_roll_supersedes_an_apply() {
        let mut queue = Vec::new();
        merge_into(&mut queue, &["ops".into(), "eng".into()], Mode::Apply);
        merge_into(&mut queue, &["ops".into()], Mode::Apply);
        assert_eq!(queue, q(&[("ops", Mode::Apply), ("eng", Mode::Apply)]));
        // A roll asked for a department already queued to apply rolls it,
        // in the place it already held — once.
        merge_into(&mut queue, &["eng".into(), "fin".into()], Mode::Roll);
        assert_eq!(
            queue,
            q(&[
                ("ops", Mode::Apply),
                ("eng", Mode::Roll),
                ("fin", Mode::Roll)
            ])
        );
        // An apply never downgrades a queued roll.
        merge_into(&mut queue, &["fin".into()], Mode::Apply);
        assert_eq!(queue[2], ("fin".to_string(), Mode::Roll));
    }

    #[test]
    fn each_server_counts_its_own_tool_list_changes() {
        assert_eq!(tool_list_count_key("github"), "mcp.tool_list_count:github");
        assert_eq!(
            tool_list_count_key("app-crm"),
            format!("{TOOL_LIST_COUNT_PREFIX}app-crm")
        );
        // A bump naming one server twice counts it once.
        assert_eq!(
            dedup(&["a".into(), "b".into(), "a".into()]),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
