// /api/channels/{id}/agents.
// POST { model } → add a fleet agent to the channel (the adder needs access
// to that agent). DELETE { model } → remove it. Any member.

use super::{ChannelNeed, channel_gate};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::fleet::usable_agent_gate;
use talaria_body::string_member;
use talaria_channels::{add_channel_agent, remove_channel_agent};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !channel_gate(&state, &user.id, &id, ChannelNeed::Member, " on agents").await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let model = match string_member(obj, "model", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // Personal assistants may join shared channels — but only their OWNER
    // can bring them (the usable-agent gate enforces that), and their group
    // replies carry the privacy gate (channel-replies): the owner's private
    // context never surfaces outside a DM with the owner.
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => return Ok(internal("[channels] agent access read failed", e)),
    };
    if !gate(&model) {
        return Ok(house_error(
            StatusCode::FORBIDDEN,
            "forbidden: no access to this agent",
        ));
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = add_channel_agent(&notify, &id, &model).await {
        return Ok(internal("[channels] agent add failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !channel_gate(&state, &user.id, &id, ChannelNeed::Member, " on agents").await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let model = match string_member(obj, "model", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = remove_channel_agent(&notify, &id, &model).await {
        return Ok(internal("[channels] agent remove failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}
