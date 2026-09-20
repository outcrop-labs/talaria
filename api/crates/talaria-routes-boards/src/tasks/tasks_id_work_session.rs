// GET /api/tasks/{id}/work-session. The LIVE WORK SESSION on a ticket, for
// the detail view's ticker and its watch modal: the run row (state, phase,
// the agent working it, the turn count) plus the tail of the agent's last
// reply, read from the checkpoint the session persists before acting. This
// is the "someone is on it" surface — the modal streams the run's own SSE
// (/api/runs/{id}/events) for live phase, so this read is the ATTACH, not
// the stream.
//
// GATED LIKE THE TASK READ (board visibility): the ticker shows on the
// ticket, so everyone who can see the ticket may see that work is happening
// on it. 404 for unknown task, `null` session when none is live.

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
    Path(id): Path<String>,
) -> Response {
    if let Some(gate) = talaria_params::uuid_gate("tasks", "GET work-session", &id) {
        return gate;
    }
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // The task's board decides, exactly like every other read of the ticket.
    let board: Option<(String,)> =
        sqlx::query_as("select board_id::text from tasks where id = $1::uuid")
            .bind(&id)
            .fetch_optional(&state.pg)
            .await
            .unwrap_or(None);
    let Some((board_id,)) = board else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    match board_role(&state.pg, &user.id, &board_id).await {
        Ok(Some(_)) => {}
        // Forbidden, not 404: the task's existence is already established
        // for this caller by the ticket being rendered — this gate only
        // decides visibility of the WORK state.
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[work-session] role read failed: {e}");
            return thrown_internal_error();
        }
    }

    // THE LIVE SESSION: newest non-terminal work-session run on this task.
    // Terminal runs are history (the ticket strip tells that story); this
    // surface answers one question — is anyone on it right now.
    let row = sqlx::query(
        "select id::text, state::text, phase, input, checkpoint \
         from runs \
         where kind = 'work-session' and subject_id = $1 \
           and state in ('queued', 'running', 'awaiting') \
         order by created_at desc limit 1",
    )
    .bind(&id)
    .fetch_optional(&state.pg)
    .await;
    let row = match row {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[work-session] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let wait = talaria_work_wait::for_task(&state.pg, &id)
        .await
        .map(|w| talaria_work_wait::wire(&w));
    let Some(row) = row else {
        return Json(json!({ "session": null, "wait": wait })).into_response();
    };
    let run_id: String = row.get("id");
    let run_state: String = row.get("state");
    let phase: Option<String> = row.get("phase");
    let input: serde_json::Value = row.get("input");
    let checkpoint: serde_json::Value = row.get("checkpoint");
    Json(json!({
        "session": {
            "runId": run_id,
            "state": run_state,
            "phase": phase,
            "agentModel": input.get("agentModel").and_then(|v| v.as_str()),
            "turn": checkpoint.get("turn").and_then(|v| v.as_i64()),
            "lastTail": checkpoint.get("lastTail").and_then(|v| v.as_str()),
        },
        "wait": wait,
    }))
    .into_response()
}
