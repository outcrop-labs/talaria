// /api/channels/{id}/messages/{msgId} — port of
// ui/src/routes/api/channels.$id.messages.$msgId.ts.
// PATCH { content } → edit your own message (edited marker shows).
// DELETE → remove it: the author, or the channel owner tidying up. A thread
// root takes its replies with it.

use crate::body::{as_object, trimmed_string_member};
use crate::channels::{
    channel_role, delete_channel_message, edit_channel_message, get_channel_message,
};
use crate::error::house_error;
use crate::notify::NotifyDeps;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, msg_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // Any role at all. The finer gate is the author check below.
    if !any_role(&state, &user.id, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let msg = match get_channel_message(&state.pg, &id, &msg_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[channels] message read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(msg) = msg else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let author = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    if msg.author_type != "user" || msg.author != author {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    // The body parses only after the author gate — zod's complaints come last.
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let content = match trimmed_string_member(obj, "content", 1, 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = edit_channel_message(&notify, &id, &msg_id, &content).await {
        tracing::error!("[channels] message edit failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, msg_id)): Path<(String, String)>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let role = match channel_role(&state.pg, &user.id, &id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[channels] role read on message delete failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(role) = role else {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    };
    let msg = match get_channel_message(&state.pg, &id, &msg_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[channels] message read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(msg) = msg else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let author = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    let own = msg.author_type == "user" && msg.author == author;
    if !own && role != "owner" {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = delete_channel_message(&notify, &id, &msg_id).await {
        tracing::error!("[channels] message delete failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    Json(json!({ "ok": true })).into_response()
}

async fn any_role(state: &AppState, user_id: &str, id: &str) -> bool {
    match channel_role(&state.pg, user_id, id).await {
        Ok(r) => r.is_some(),
        Err(e) => {
            tracing::error!("[channels] role read on message patch failed: {e}");
            false
        }
    }
}
