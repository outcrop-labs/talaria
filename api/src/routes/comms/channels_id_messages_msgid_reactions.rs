// /api/channels/{id}/messages/{msgId}/reactions.
// POST { emoji } → toggle your reaction on a message. Agents react too, under
// their own identity — one of our twists on the Slack shape.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::body::{as_object, string_member};
use crate::channels::{
    agent_may_access_channel, channel_role, get_channel_message, toggle_reaction,
};
use crate::error::{house_error, thrown_internal_error};
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
    Path((id, msg_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Response {
    // The body parses FIRST — a malformed emoji outranks every gate.
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let emoji = match string_member(obj, "emoji", 1, 16) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let msg = match get_channel_message(&state.pg, &id, &msg_id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[channels] message read on reactions failed: {e}");
            return thrown_internal_error();
        }
    };
    if msg.is_none() {
        return house_error(StatusCode::NOT_FOUND, "not found");
    }

    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Some(caller) = caller {
        let name = caller.model.clone();
        // The CALLER, not `name`: elevation buys org-wide channel reach.
        let may =
            match agent_may_access_channel(&state.pg, &id, &AgentSubject::Caller(caller)).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("[channels] agent access read on reactions failed: {e}");
                    return thrown_internal_error();
                }
            };
        if !may {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        return match toggle_reaction(&notify, &id, &msg_id, &emoji, &name, "agent").await {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(e) => {
                tracing::error!("[channels] agent reaction failed: {e}");
                thrown_internal_error()
            }
        };
    }
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on reactions failed: {e}");
            return thrown_internal_error();
        }
    }
    let actor = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    match toggle_reaction(&notify, &id, &msg_id, &emoji, &actor, "user").await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => {
            tracing::error!("[channels] reaction failed: {e}");
            thrown_internal_error()
        }
    }
}
