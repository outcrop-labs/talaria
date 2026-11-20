// POST /api/tasks/{id}/resync — "Re-sync from ClickUp": the ticket's ClickUp
// task is read again and forced onto it. Owner or editor on the board, for a
// ticket its live connection syncs; 409 while a sync runs on the board, while
// the sync is paused or waits for a new credential. Answers {runId}. Forced:
// title, description, status through the sync's column guards, priority,
// dates; ClickUp's labels and human assignees replace the ticket's; agent
// assignees are never touched.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_board_sync::ops;
use talaria_session::require_user;
use talaria_state::AppState;

use crate::boards::boards_id_sync::op_error;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let id = match super::resolve_task_path(&state.pg, &id).await {
        Ok(id) => id,
        Err(resp) => return Ok(resp),
    };
    match ops::resync_ticket(&state, &id, &user.id).await {
        Ok(run_id) => Ok(Json(json!({ "runId": run_id })).into_response()),
        Err(e) => Ok(op_error(e, "[board-sync] re-sync failed")),
    }
}
