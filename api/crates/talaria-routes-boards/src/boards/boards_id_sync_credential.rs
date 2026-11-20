// PUT /api/boards/{id}/sync/credential {token} — replace the sync's ClickUp
// token. Owner only; the new token must read the connected list. A sync that
// waits for a credential turns active and runs at once — or returns to paused
// when it was paused as the token died. Answers {sync}. For when the member
// who connected it left, or ClickUp refused it; the mapping, the links and
// the history stay.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_board_sync::ops;
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

use super::boards_id_sync::op_error;

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("boards", "PUT sync/credential", &id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let token = match string_member(obj, "token", 8, 400) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(e) = ops::replace_credential(&state, &id, &user.id, &token).await {
        return Ok(op_error(e, "[board-sync] credential replace failed"));
    }
    let body =
        talaria_board_sync::status::board_sync_status(&state.pg, &id, talaria_agent_auth::now_ms())
            .await
            .map_err(|e| internal("[board-sync] status read failed", e))?;
    Ok(Json(json!({ "sync": body["sync"] })).into_response())
}
