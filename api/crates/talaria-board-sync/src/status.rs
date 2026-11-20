// The connection as the board reads it — GET /api/boards/{id}/sync. One
// projection of the board_syncs row, its runs and its items, never a second
// store: last and next sync, the live run's phase, the problems a person can
// act on, and whether the schedule can run at all on this instance.
//
// NEVER SILENTLY STALE: `overdue` is an active, scheduled connection whose
// next sync is five minutes past with nothing running; `schedulerArmed` false
// says this instance runs no schedule (TALARIA_SCHEDULER=off), so only Sync
// now syncs; a connector no longer on the board is a warning, because their
// ClickUp token still drives the sync.

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;

use crate::store::{SyncRow, item_problems, live_sync, remote_states};
use crate::{list_url, live_run};

pub const PROBLEMS_CAP: i64 = 50;
const OVERDUE_MS: i64 = 5 * 60 * 1000;

fn iso(ms: Option<i64>) -> Value {
    ms.map(|m| Value::String(epoch_ms_to_iso(m)))
        .unwrap_or(Value::Null)
}

/// The derived liveness, pure so it is tested against a fixed clock.
pub fn overdue(sync: &SyncRow, live: bool, now: i64) -> bool {
    sync.state == "active"
        && sync.interval_minutes.is_some()
        && !live
        && sync.next_sync_ms.is_some_and(|n| now - n > OVERDUE_MS)
}

async fn last_run(pg: &PgPool, id: &str) -> Result<Value, sqlx::Error> {
    let row: Option<(String, String, Option<String>, Option<Value>, Option<i64>)> = sqlx::query_as(
        "select id::text, state, error, result, (trunc(extract(epoch from finished_at) * 1000))::bigint \
         from runs where id = $1::uuid",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(match row {
        Some((id, state, error, result, finished)) => json!({
            "id": id,
            "state": state,
            "error": error,
            "result": result,
            "finishedAt": iso(finished),
        }),
        None => Value::Null,
    })
}

async fn connector(pg: &PgPool, sync: &SyncRow) -> Result<Value, sqlx::Error> {
    let Some(uid) = &sync.connected_by else {
        return Ok(Value::Null);
    };
    let row: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as("select id::text, name, email from users where id = $1::uuid")
            .bind(uid)
            .fetch_optional(pg)
            .await?;
    let still = talaria_boards::board_role(pg, uid, &sync.board_id)
        .await?
        .is_some();
    Ok(match row {
        Some((id, name, email)) => {
            json!({ "id": id, "name": name, "email": email, "onBoard": still })
        }
        None => Value::Null,
    })
}

/// `{sync, disconnected}`: the live connection's projection (or null), and —
/// when there is none — the last one this board had, so its imported tickets
/// can say where they came from.
pub async fn board_sync_status(
    pg: &PgPool,
    board_id: &str,
    now: i64,
) -> Result<Value, sqlx::Error> {
    let Some(sync) = live_sync(pg, board_id).await? else {
        let last: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
            "select list_path, (trunc(extract(epoch from disconnected_at) * 1000))::bigint \
             from board_syncs where board_id = $1::uuid and state = 'disconnected' \
             order by disconnected_at desc nulls last limit 1",
        )
        .bind(board_id)
        .fetch_optional(pg)
        .await?;
        return Ok(json!({
            "sync": null,
            "disconnected": last.map(|(path, at)| json!({ "listPath": path, "disconnectedAt": iso(at) })),
        }));
    };
    let live = live_run(pg, board_id).await?;
    let last = match &sync.last_run_id {
        Some(id) => last_run(pg, id).await?,
        None => Value::Null,
    };
    let connected_by = connector(pg, &sync).await?;
    let mut warnings: Vec<Value> = Vec::new();
    if connected_by.get("onBoard") == Some(&Value::Bool(false)) {
        warnings.push(json!({
            "kind": "connector-left",
            "detail": "The person who connected this sync is no longer on the board, and their ClickUp token still drives it",
            "fix": "A board owner replaces the credential in Board settings → Sync",
        }));
    }
    let scheduler_armed = !talaria_scheduler::scheduler_disabled();
    if !scheduler_armed && sync.interval_minutes.is_some() {
        warnings.push(json!({
            "kind": "schedule-off",
            "detail": "Scheduled syncs are off on this instance",
            "fix": "Use Sync now",
        }));
    }
    let (items, item_total) = item_problems(pg, &sync.id, PROBLEMS_CAP).await?;
    let mut problems: Vec<Value> = sync
        .run_problems
        .iter()
        .map(|p| serde_json::to_value(p).unwrap_or(Value::Null))
        .collect();
    let run_total = problems.len() as i64;
    for p in items {
        if problems.len() as i64 >= PROBLEMS_CAP {
            break;
        }
        problems.push(serde_json::to_value(p).unwrap_or(Value::Null));
    }
    let account = sync.credential_account.clone().unwrap_or(Value::Null);
    Ok(json!({
        "sync": {
            "id": sync.id,
            "provider": sync.provider,
            "state": sync.state,
            "listId": sync.list_id,
            "listPath": sync.list_path,
            "listUrl": list_url(sync.workspace_id.as_deref(), &sync.list_id),
            "workspaceId": sync.workspace_id,
            "account": account,
            "credentialHint": sync.credential_hint,
            "statusMap": sync.status_map,
            "mapVersion": sync.map_version,
            "remapPending": sync.remap_pending,
            "options": sync.options,
            "intervalMinutes": sync.interval_minutes,
            "lastSyncedAt": iso(sync.last_synced_ms),
            "nextSyncAt": iso(sync.next_sync_ms),
            "lastSweepAt": iso(sync.last_sweep_ms),
            "overdue": overdue(&sync, live.is_some(), now),
            "schedulerArmed": scheduler_armed,
            "liveRun": live.map(|(id, phase)| json!({ "id": id, "phase": phase })),
            "lastRun": last,
            "connectedBy": connected_by,
            "warnings": warnings,
            "problems": problems,
            "problemCount": run_total + item_total,
            "statuses": sync.list_statuses,
            "remoteStates": remote_states(pg, &sync.id).await?,
            "createdAt": epoch_ms_to_iso(sync.created_ms),
        },
        "disconnected": null,
    }))
}
