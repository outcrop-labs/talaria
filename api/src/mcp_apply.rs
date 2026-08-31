// Applying MCP registry changes to RUNNING agents — port of
// ui/src/server/mcp-apply.ts. A config re-render isn't enough: Hermes
// establishes its MCP connections at process start, so a newly granted server
// never appears inside a live container. The fix is the fleet's blue/green
// roll — the current container keeps serving until its replacement is
// healthy, in-flight replies drain, then cutover — applied to exactly the
// agents a change touches. Rolls run sequentially off a deduped queue: a
// burst of registry edits becomes one roll per department.

use serde_json::json;
use sqlx::PgPool;
use std::sync::{LazyLock, Mutex};

use crate::audit::{AuditEntry, log_audit};
use crate::fleet_reconcile::roll_agent;
use crate::secretbox::SecretBox;

/// The process-local roll queue, exactly TS's module state: a deduped
/// department list plus a running flag the pump clears when it drains.
static QUEUE: LazyLock<Mutex<(Vec<String>, bool)>> =
    LazyLock::new(|| Mutex::new((Vec::new(), false)));

/// The one pump. Lazily armed so a test environment with no runtime still
/// links; in the process it's the tokio handle the routes spawn under.
fn enqueue(departments: &[String], pg: &PgPool, sb: &SecretBox) {
    {
        let mut q = QUEUE.lock().unwrap_or_else(|p| p.into_inner());
        for d in departments {
            if !q.0.contains(d) {
                q.0.push(d.clone());
            }
        }
        if q.1 {
            return;
        }
        q.1 = true;
    }
    let pg = pg.clone();
    let sb = sb.clone();
    tokio::spawn(async move {
        loop {
            let next = {
                let mut q = QUEUE.lock().unwrap_or_else(|p| p.into_inner());
                if q.0.is_empty() {
                    q.1 = false;
                    return;
                }
                q.0.remove(0)
            };
            let rolled = roll_agent(&pg, &sb, &next).await;
            // `{ok:false}` AND a thrown step both land here: the roll verdict
            // is a sentence for the operator's audit trail, never a caller's
            // problem — the mutation that triggered it is already committed.
            let error = match rolled {
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
                    target_id: Some(&next),
                    target_label: None,
                    before: None,
                    after: Some(json!({ "error": error })),
                },
            )
            .await;
        }
    });
}

/// The departments whose managed agents carry this server right now.
pub async fn carriers_for_server(
    pg: &PgPool,
    server_id: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let all_agents: Option<(bool,)> =
        sqlx::query_as("select all_agents from mcp_servers where id::text = $1")
            .bind(server_id)
            .fetch_optional(pg)
            .await?;
    let rows: Vec<(String,)> = if all_agents.is_some_and(|(a,)| a) {
        sqlx::query_as(
            "select distinct department from agent_defs where managed and enabled",
        )
        .fetch_all(pg)
        .await?
    } else {
        sqlx::query_as(
            "select distinct d.department from mcp_server_agents a \
             join agent_defs d on d.model = a.agent_model and d.managed and d.enabled \
             where a.server_id::text = $1",
        )
        .bind(server_id)
        .fetch_all(pg)
        .await?
    };
    Ok(rows.into_iter().map(|(d,)| d).collect())
}

/// Queue rolls for an explicit department list (e.g. captured pre-delete).
pub fn enqueue_rolls(departments: &[String], pg: &PgPool, sb: &SecretBox) {
    enqueue(departments, pg, sb);
}

/// Roll every managed agent that carries this server (all-agents or assigned).
pub async fn roll_agents_for_server(pg: &PgPool, sb: &SecretBox, server_id: &str) {
    if let Ok(departments) = carriers_for_server(pg, server_id).await {
        enqueue(&departments, pg, sb);
    }
}

/// Roll one user's personal assistant (their connect/disconnect took effect).
pub async fn roll_agent_for_user(pg: &PgPool, sb: &SecretBox, user_id: &str) {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select department from agent_defs where owner_user_id = $1::uuid and managed and enabled",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    let departments = rows.into_iter().map(|(d,)| d).collect::<Vec<_>>();
    enqueue(&departments, pg, sb);
}

/// Roll one specific agent model's department.
pub async fn roll_agent_for_model(pg: &PgPool, sb: &SecretBox, model: &str) {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select department from agent_defs where model = $1 and managed and enabled",
    )
    .bind(model)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    let departments = rows.into_iter().map(|(d,)| d).collect::<Vec<_>>();
    enqueue(&departments, pg, sb);
}
