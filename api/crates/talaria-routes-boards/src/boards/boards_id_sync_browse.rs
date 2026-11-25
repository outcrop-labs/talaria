// POST /api/boards/{id}/sync/browse {token?, teamId?, spaceId?, folderId?} —
// one level of the ClickUp hierarchy a person walks to pick a list, lazily:
// no ids → the token's account and workspaces; teamId → its spaces; spaceId
// → its folders and folderless lists; folderId → its lists. `token` is
// required until the board is connected (then the stored credential serves).
// Owner only. A read of ClickUp, so POST only because the token rides in the
// body, never a URL.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use talaria_board_sync::ops::{self, BrowseInput};
use talaria_body::{optional_string_member, parse};
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
    if let Some(gate) = talaria_params::uuid_gate("boards", "POST sync/browse", &id) {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let mut input = BrowseInput::default();
    for (key, slot) in [
        ("token", &mut input.token),
        ("teamId", &mut input.team_id),
        ("spaceId", &mut input.space_id),
        ("folderId", &mut input.folder_id),
    ] {
        match optional_string_member(obj, key, 400) {
            Ok(v) => *slot = v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    }
    match ops::browse(&state, &id, &user.id, input).await {
        Ok(v) => Ok(Json(v).into_response()),
        Err(e) => Ok(op_error(e, "[board-sync] browse failed")),
    }
}
