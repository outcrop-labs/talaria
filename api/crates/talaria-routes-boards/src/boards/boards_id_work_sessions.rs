// GET /api/boards/{id}/work-sessions. The board's LIVE WORK SESSIONS as a
// taskId → session map — the list view's question ("which cards are being
// worked right now?") answered in one read instead of one per card. Gated by
// board visibility exactly like the board read itself; the map carries only
// what a card renders (agent, turn, phase), never reply content.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::Row;

use talaria_boards::board_role;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(board_id): Path<String>,
) -> Result<Response, Response> {
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET work-sessions", &board_id) {
        return Ok(gate);
    }
    let user = require_user(&state, &headers).await?;
    match board_role(&state.pg, &user.id, &board_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[boards/work-sessions] role read failed", e)),
    }

    // Every non-terminal work session on this board's tasks, newest per
    // task. The lateral-limited join keeps one row per task without a
    // window function: the newest live run per subject is the session.
    let rows = sqlx::query(
        "select distinct on (r.subject_id) \
            r.subject_id::text, r.id::text, r.state::text, r.phase, r.input, r.checkpoint, \
            r.agent_run_id, (trunc(extract(epoch from r.created_at) * 1000))::bigint as since_ms \
         from runs r \
         join tasks t on t.id::text = r.subject_id \
         where r.kind = 'work-session' \
           and r.state in ('queued', 'running', 'awaiting') \
           and t.board_id = $1::uuid \
         order by r.subject_id, r.created_at desc",
    )
    .bind(&board_id)
    .fetch_all(&state.pg)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return Ok(internal("[boards/work-sessions] read failed", e)),
    };
    let mut map = serde_json::Map::new();
    let mut live = Vec::with_capacity(rows.len());
    for row in rows {
        let input: serde_json::Value = row.get("input");
        let checkpoint: serde_json::Value = row.get("checkpoint");
        let subject_id: String = row.get("subject_id");
        let run_id: String = row.get("id");
        let run_state: String = row.get("state");
        let phase: Option<String> = row.get("phase");
        let agent_model = input
            .get("agentModel")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let since_ms: i64 = row.get("since_ms");
        live.push(crate::work_status::Live {
            run_id: run_id.clone(),
            task_id: subject_id.clone(),
            agent_model: agent_model.clone(),
            since_ms,
            agent_run: row
                .get::<Option<String>, _>("agent_run_id")
                .filter(|s| !s.is_empty()),
        });
        map.insert(
            subject_id,
            json!({
                "runId": run_id,
                "state": run_state,
                "phase": phase,
                "agentModel": agent_model,
                "turn": checkpoint.get("turn").and_then(|v| v.as_i64()),
                "startedAt": since_ms,
            }),
        );
    }
    // One pulse read and one ledger read for every card, not one per row.
    for (l, status) in live
        .iter()
        .zip(crate::work_status::statuses(&state, &live).await)
    {
        if let Some(entry) = map.get_mut(&l.task_id) {
            entry["activity"] = status["activity"].clone();
            entry["spend"] = status["spend"].clone();
            entry["now"] = status["now"].clone();
        }
    }
    let mut waits = serde_json::Map::new();
    let mut behind: std::collections::HashMap<String, serde_json::Value> = Default::default();
    for (task_id, wait) in talaria_work_wait::for_board(&state.pg, &board_id).await {
        if !map.contains_key(&task_id) {
            let mut wire = talaria_work_wait::wire(&wait);
            if !behind.contains_key(&wait.agent_model) {
                let refs = talaria_work_wait::working_on(&state.pg, &wait.agent_model).await;
                behind.insert(wait.agent_model.clone(), json!(refs));
            }
            wire["behind"] = behind[&wait.agent_model].clone();
            waits.insert(task_id, wire);
        }
    }
    Ok(Json(json!({ "sessions": map, "waits": waits })).into_response())
}
