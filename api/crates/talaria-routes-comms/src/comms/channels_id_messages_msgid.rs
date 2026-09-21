// /api/channels/{id}/messages/{msgId}.
// PATCH { content } → edit your own message (edited marker shows).
// DELETE → remove it: the author, or the channel owner tidying up. A thread
// root takes its replies with it.

use super::{ChannelNeed, channel_gate};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::trimmed_string_member;
use talaria_channels::{
    channel_role, delete_channel_message, edit_channel_message, get_channel_message,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, msg_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // Any role at all. The finer gate is the author check below.
    if !channel_gate(
        &state,
        &user.id,
        &id,
        ChannelNeed::Member,
        " on message patch",
    )
    .await
    {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let msg = match get_channel_message(&state.pg, &id, &msg_id).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[channels] message read failed", e)),
    };
    let Some(msg) = msg else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let author = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    if msg.author_type != "user" || msg.author != author {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    // The body parses only after the author gate — validation errors come last.
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let content = match trimmed_string_member(obj, "content", 1, 20_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = edit_channel_message(&notify, &id, &msg_id, &content).await {
        return Ok(internal("[channels] message edit failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, msg_id)): Path<(String, String)>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let role = match channel_role(&state.pg, &user.id, &id).await {
        Ok(r) => r,
        Err(e) => return Ok(internal("[channels] role read on message delete failed", e)),
    };
    let Some(role) = role else {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    };
    let msg = match get_channel_message(&state.pg, &id, &msg_id).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[channels] message read failed", e)),
    };
    let Some(msg) = msg else {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    };
    let author = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    let own = msg.author_type == "user" && msg.author == author;
    if !own && role != "owner" {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Err(e) = delete_channel_message(&notify, &id, &msg_id).await {
        return Ok(internal("[channels] message delete failed", e));
    }
    Ok(Json(json!({ "ok": true })).into_response())
}
