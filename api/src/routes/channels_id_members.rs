// /api/channels/{id}/members — port of ui/src/routes/api/channels.$id.members.ts.
// POST { email } → add a member (any member can invite; they must have
// signed in before — the engine answers the no-show sentence as a 400).
// DELETE { userId } → remove a member (owner, or yourself to leave).

use crate::body::{as_object, email_member, uuid_member};
use crate::channels::{add_channel_member, channel_role, remove_channel_member};
use crate::error::house_error;
use crate::notify::NotifyDeps;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !member(&state, &user.id, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let email = match email_member(obj, "email") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    match add_channel_member(&notify, &id, &email).await {
        Ok(None) => Json(json!({ "ok": true })).into_response(),
        // The engine's own sentence ("No user with that email has signed in
        // yet") rides the 400, exactly as `res.ok ? … : {error: res.error}`.
        Ok(Some(error)) => house_error(StatusCode::BAD_REQUEST, &error),
        Err(e) => {
            tracing::error!("[channels] member add failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let role = match channel_role(&state.pg, &user.id, &id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[channels] role read on member delete failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    let Some(role) = role else {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    };
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let user_id = match uuid_member(obj, "userId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if role != "owner" && user_id != user.id {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = remove_channel_member(&notify, &id, &user_id).await {
        tracing::error!("[channels] member remove failed: {e}");
        return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    Json(json!({ "ok": true })).into_response()
}

async fn member(state: &AppState, user_id: &str, id: &str) -> bool {
    match channel_role(&state.pg, user_id, id).await {
        Ok(r) => r.is_some(),
        Err(e) => {
            tracing::error!("[channels] role read on members failed: {e}");
            false
        }
    }
}
