// /api/artifacts/{id}/duplicate. Copy one artifact — copy/paste's engine on
// the file side. A READ-level act (anyone who can see it can copy it; the
// source is never touched): the copy is the caller's, private, beside its
// source, under a "Copy of" name. See artifacts::duplicate_artifact for the
// full copy semantics (shared storage ref, no public/KB/Google linkage).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::artifacts::{duplicate_artifact, get_artifact, guarded};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::perms::{ITEM_ARTIFACT, can_read, list_editors};
use crate::session::{require_user, who_of};
use crate::state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let src = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(src) = src else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &src.id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[artifacts] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    if !can_read(
        &guarded(&src),
        Some(&user.id),
        who_of(&user).as_deref(),
        &editors,
    ) {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match duplicate_artifact(&state.pg, &src.id, &user.id, Some(&user.id)).await {
        Ok(Some(copy)) => Json(json!({ "artifact": copy })).into_response(),
        Ok(None) => house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[artifacts] duplicate failed: {e}");
            thrown_internal_error()
        }
    }
}
