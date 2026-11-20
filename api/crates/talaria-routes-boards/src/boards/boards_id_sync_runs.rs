// POST /api/boards/{id}/sync/runs — Sync now: start a sync of the board's
// ClickUp list (it always reconciles every task), or answer the run already
// going. Owner or editor. Answers {runId, live}: `live` true when the run
// was already under way. 409 while the sync waits for a new credential.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_board_sync::{Started, ops};
use talaria_session::require_user;
use talaria_state::AppState;

use super::boards_id_sync::op_error;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "POST sync/runs", &id) {
        return Ok(gate);
    }
    match ops::sync_now(&state, &id, &user.id).await {
        Ok(started) => Ok(Json(json!({
            "runId": started.id(),
            "live": matches!(started, Started::Live(_)),
        }))
        .into_response()),
        Err(e) => Ok(op_error(e, "[board-sync] sync now failed")),
    }
}
