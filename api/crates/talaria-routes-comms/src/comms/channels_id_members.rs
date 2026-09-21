// /api/channels/{id}/members.
// POST { email } → add a member (any member can invite; they must have
// signed in before — the engine answers the no-show sentence as a 400).
// DELETE { userId } → remove a member (owner, or yourself to leave).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::{as_object, email_member, uuid_member};
use talaria_channels::{add_channel_member, channel_role, remove_channel_member};
use talaria_error::{house_error, internal};
use talaria_notify::NotifyDeps;
use talaria_session::require_user;
use talaria_state::AppState;

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
    let parsed = talaria_body::parse(&body);
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
        // yet") rides the 400 body verbatim.
        Ok(Some(error)) => house_error(StatusCode::BAD_REQUEST, &error),
        Err(e) => internal("[channels] member add failed", e),
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
        Err(e) => return internal("[channels] role read on member delete failed", e),
    };
    let Some(role) = role else {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    };
    let parsed = talaria_body::parse(&body);
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
        return internal("[channels] member remove failed", e);
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
