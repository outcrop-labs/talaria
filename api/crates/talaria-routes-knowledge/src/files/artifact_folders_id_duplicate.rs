// /api/artifact-folders/{id}/duplicate. Copy a whole folder tree in one
// transaction — copy/paste's engine on the folder side. Read-level like the
// artifact twin: anyone who can read the folder can copy it; the copy (all of
// it) becomes the caller's, folders keep their visibility (they are org-wide
// containers), artifacts land private. Children keep their names — the folder
// is new, nothing collides.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::kb::perms::{can_read, list_editors};
use talaria_artifacts::{duplicate_folder, get_folder, guarded_folder};
use talaria_error::{house_error, internal};
use talaria_session::{require_user, who_of};
use talaria_state::AppState;

const ITEM_FOLDER: &str = "artifact-folder";

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let src = match get_folder(&state.pg, &id).await {
        Ok(f) => f,
        Err(e) => return internal("[folders] read failed", e),
    };
    let Some(src) = src else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let editors = match list_editors(&state.pg, ITEM_FOLDER, &src.id).await {
        Ok(e) => e,
        Err(e) => return internal("[folders] grants read failed", e),
    };
    // The read gate the GET uses, owner arm included.
    let team_ids = match talaria_teams::team_ids_for_user(&state.pg, &user.id).await {
        Ok(v) => v,
        Err(e) => return internal("[folders] team membership read failed", e),
    };
    if !can_read(
        &guarded_folder(&src),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
        &team_ids,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match duplicate_folder(&state.pg, &src.id, &user.id, Some(&user.id)).await {
        Ok(Some(copy)) => Json(json!({ "folder": copy })).into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => internal("[folders] duplicate failed", e),
    }
}
