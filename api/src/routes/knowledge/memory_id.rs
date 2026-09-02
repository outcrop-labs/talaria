// /api/memory/{id}. One managed agent's MEMORY.md, read/written through its
// running container. Writes: admin, or the owner of a personal assistant for
// its own memory.

use crate::agent_memory::{read_memory, write_memory};
use crate::body::{as_object, parse, string_member};
use crate::error::house_error;
use crate::personal_agent::owns_agent;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The gate both verbs share: an agent's memory is its owner's (or admin's)
/// business — a personal assistant's memory is dense private context.
async fn allowed(state: &AppState, user_id: &str, role: &str, def_id: &str) -> bool {
    role == "admin" || owns_agent(&state.pg, user_id, None, Some(def_id)).await
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !allowed(&state, &user.id, &user.role, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match read_memory(&state.pg, &id).await {
        Ok((content, container)) => {
            Json(json!({ "content": content, "container": container })).into_response()
        }
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !allowed(&state, &user.id, &user.role, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // content — required, max 2M; the empty string is legal (min 0: clearing
    // a memory is a write).
    let content = match string_member(obj, "content", 0, 2_000_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let author = user
        .email
        .as_deref()
        .or(user.name.as_deref())
        .unwrap_or("admin");
    match write_memory(&state.pg, &id, &content, Some(author)).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}
