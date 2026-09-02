// /api/agent/message-user. POST (agent key) → an agent starts or continues a direct conversation with
// a human teammate. The message lands as a normal turn in their chat with
// this agent plus an inbox notification. Personal assistants reach only
// their owner; every agent↔user pair is rate-capped per day.

use crate::agent_auth::require_agent;
use crate::body::{as_object, parse, string_member};
use crate::error::house_error;
use crate::notify::NotifyDeps;
use crate::outreach::agent_message_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let caller = match require_agent(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Teammate's email (preferred) or exact display name.
    let to = match string_member(obj, "to", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let message = match string_member(obj, "message", 1, 4000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let deps = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    let result = agent_message_user(&deps, &caller.model, &to, &message).await;
    if !result.ok {
        let line = result.error.unwrap_or_default();
        return house_error(StatusCode::BAD_REQUEST, &line);
    }
    Json(json!({ "ok": true, "conversationId": result.conversation_id })).into_response()
}
