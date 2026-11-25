// POST /api/boards/{id}/sync/preview {token?, listId} — what connecting would
// do, before anything is written: the list's statuses with their suggested
// columns (exact label matches, or this board's last mapping of the list —
// nothing guessed), the columns on offer and which start agent work, the
// first tasks as they would land, who would and would not be assigned, the
// labels it would create, oversized files in the sample, and an estimate of
// the first import at this token's rate limit. Statistics come from up to the
// first 500 tasks (`statsFrom` says how many). Owner only; no writes.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_board_sync::ops;
use talaria_body::{optional_string_member, parse, string_member};
use talaria_error::{house_error, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

use super::boards_id_sync::op_error;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "POST sync/preview", &id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let token = match optional_string_member(obj, "token", 400) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let list_id = match string_member(obj, "listId", 1, 100) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match ops::preview(&state, &id, &user.id, token.as_deref(), &list_id).await {
        Ok(v) => Ok(Json(v).into_response()),
        Err(e) => Ok(op_error(e, "[board-sync] preview failed")),
    }
}
