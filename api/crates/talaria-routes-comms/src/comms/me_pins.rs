// /api/me/pins. The caller's Comms rail pins — what leads their rail, in the
// order they pinned it. GET → { pins: ["channel:<id>" | "agent:<model>" |
// "thread:<id>", …] }, oldest first. POST { key } pins one (idempotent: a key
// already pinned keeps its place) → { ok: true }; a person holds at most 200,
// and a 201st is a 409. DELETE { key } unpins → { ok: true }, whether or not
// it was pinned. The key grammar lives in talaria_conversation_layout::pins.
// A pin is the caller's own filing, so it needs no access check on what it
// names: the rail only draws pins for rows it already lists.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::string_member;
use talaria_conversation_layout::pins::{
    MAX_PINS, PinAdd, add_pin, list_pins, parse_pin_key, remove_pin,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

/// `channel:` or `thread:` plus a uuid is 43 characters; an agent model may
/// run to 200.
const MAX_KEY: usize = 220;

/// The validated `key` member, or the 400 sentence.
fn key_of(obj: &serde_json::Map<String, serde_json::Value>) -> Result<String, String> {
    parse_pin_key(&string_member(obj, "key", 1, MAX_KEY)?)
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match list_pins(&state.pg, &user.id).await {
        Ok(pins) => Json(json!({ "pins": pins })).into_response(),
        Err(e) => internal("[me/pins] read failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let key = match key_of(obj) {
        Ok(k) => k,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(match add_pin(&state.pg, &user.id, &key).await {
        Ok(PinAdd::Pinned) => Json(json!({ "ok": true })).into_response(),
        Ok(PinAdd::Full) => house_error(
            StatusCode::CONFLICT,
            &format!("you can pin up to {MAX_PINS} conversations — unpin one first"),
        ),
        Err(e) => internal("[me/pins] pin failed", e),
    })
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let key = match key_of(obj) {
        Ok(k) => k,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(match remove_pin(&state.pg, &user.id, &key).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => internal("[me/pins] unpin failed", e),
    })
}
