// /api/channels/{id}/agents — port of ui/src/routes/api/channels.$id.agents.ts.
// POST { model } → add a fleet agent to the channel (the adder needs access
// to that agent). DELETE { model } → remove it. Any member.

use crate::body::{as_object, string_member};
use crate::channels::{add_channel_agent, channel_role, remove_channel_agent};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::usable_agent_gate;
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
    if !member_gate(&state, &user.id, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let model = match string_member(obj, "model", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Personal assistants may join shared channels — but only their OWNER
    // can bring them (the usable-agent gate enforces that), and their group
    // replies carry the privacy gate (channel-replies): the owner's private
    // context never surfaces outside a DM with the owner.
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[channels] agent access read failed: {e}");
            return thrown_internal_error();
        }
    };
    if !gate(&model) {
        return house_error(StatusCode::FORBIDDEN, "forbidden: no access to this agent");
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = add_channel_agent(&notify, &id, &model).await {
        tracing::error!("[channels] agent add failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
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
    if !member_gate(&state, &user.id, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let model = match string_member(obj, "model", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = remove_channel_agent(&notify, &id, &model).await {
        tracing::error!("[channels] agent remove failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

/// Any role at all (member or owner) admits the call.
async fn member_gate(state: &AppState, user_id: &str, id: &str) -> bool {
    match channel_role(&state.pg, user_id, id).await {
        Ok(r) => r.is_some(),
        Err(e) => {
            tracing::error!("[channels] role read on agents failed: {e}");
            false
        }
    }
}
