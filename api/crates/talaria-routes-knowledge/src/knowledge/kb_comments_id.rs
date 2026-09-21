// /api/kb/comments/{id}. One comment. PATCH { resolved } → resolve/unresolve
// its thread (author, thread starter, or doc owner). DELETE → remove your own
// comment.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_api_facades::kb::comments::{delete_comment, set_resolved};
use talaria_body::{boolean_member, parse};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let resolved = match boolean_member(obj, "resolved") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
        match set_resolved(&state.pg, &id, resolved, &user.id).await {
            Ok(true) => Json(json!({ "ok": true })).into_response(),
            Ok(false) => house_error(StatusCode::FORBIDDEN, "forbidden"),
            Err(e) => internal("[kb] resolve failed", e),
        },
    )
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match delete_comment(&state.pg, &id, &user.id).await {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => internal("[kb] comment delete failed", e),
    })
}
