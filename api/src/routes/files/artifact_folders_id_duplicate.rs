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

use crate::artifacts::{duplicate_folder, get_folder, guarded_folder};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{can_read, list_editors};
use crate::session::{require_user, who_of};
use crate::state::AppState;

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
        Err(e) => {
            tracing::error!("[folders] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(src) = src else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let editors = match list_editors(&state.pg, ITEM_FOLDER, &src.id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[folders] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    // The read gate the GET uses, owner arm included.
    if !can_read(
        &guarded_folder(&src),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match duplicate_folder(&state.pg, &src.id, &user.id, Some(&user.id)).await {
        Ok(Some(copy)) => Json(json!({ "folder": copy })).into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[folders] duplicate failed: {e}");
            thrown_internal_error()
        }
    }
}
