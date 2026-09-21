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
use talaria_error::{house_error, thrown_internal_error};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(board_id): Path<String>,
) -> Response {
    if let Some(gate) = talaria_params::uuid_gate("boards", "GET work-sessions", &board_id) {
        return gate;
    }
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match board_role(&state.pg, &user.id, &board_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[boards/work-sessions] role read failed: {e}");
            return thrown_internal_error();
        }
    }

    // Every non-terminal work session on this board's tasks, newest per
    // task. The lateral-limited join keeps one row per task without a
    // window function: the newest live run per subject is the session.
    let rows = sqlx::query(
        "select distinct on (r.subject_id) \
            r.subject_id::text, r.id::text, r.state::text, r.phase, r.input, r.checkpoint \
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
        Err(e) => {
            tracing::error!("[boards/work-sessions] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let mut map = serde_json::Map::new();
    for row in rows {
        let input: serde_json::Value = row.get("input");
        let checkpoint: serde_json::Value = row.get("checkpoint");
        let subject_id: String = row.get("subject_id");
        let run_id: String = row.get("id");
        let run_state: String = row.get("state");
        let phase: Option<String> = row.get("phase");
        map.insert(
            subject_id,
            json!({
                "runId": run_id,
                "state": run_state,
                "phase": phase,
                "agentModel": input.get("agentModel").and_then(|v| v.as_str()),
                "turn": checkpoint.get("turn").and_then(|v| v.as_i64()),
            }),
        );
    }
    let mut waits = serde_json::Map::new();
    for (task_id, wait) in talaria_work_wait::for_board(&state.pg, &board_id).await {
        if !map.contains_key(&task_id) {
            waits.insert(task_id, talaria_work_wait::wire(&wait));
        }
    }
    Json(json!({ "sessions": map, "waits": waits })).into_response()
}
