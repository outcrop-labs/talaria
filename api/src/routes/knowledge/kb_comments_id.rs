// /api/kb/comments/{id}. One comment. PATCH { resolved } → resolve/unresolve
// its thread (author, thread starter, or doc owner). DELETE → remove your own
// comment.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::body::{as_object, boolean_member, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::kb::comments::{delete_comment, set_resolved};
use crate::session::require_user;
use crate::state::AppState;

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let resolved = match boolean_member(obj, "resolved") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match set_resolved(&state.pg, &id, resolved, &user.id).await {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[kb] resolve failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match delete_comment(&state.pg, &id, &user.id).await {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[kb] comment delete failed: {e}");
            thrown_internal_error()
        }
    }
}
