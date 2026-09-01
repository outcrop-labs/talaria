// /api/me/assistant — port of ui/src/routes/api/me.assistant.ts.
// The signed-in user's personal assistant. GET → theirs (or null), with
// personality + live status. POST → create + start one, optionally named/
// personalized (idempotent: returns the existing one, re-enabling if retired).
// PATCH → owner-scoped rename / personality edit. Any signed-in user; every
// operation is keyed on agent_defs.owner_user_id, never on a client-sent id.

use crate::body::{as_object, optional_max_string_member, parse, string_msg, zod_type_name};
use crate::error::house_error;
use crate::personal_agent::{
    HANDLE_RE_NOTE, PersonalAgentInput, PersonalAgentPatch, PersonalUser, create_personal_agent,
    handle_ok, personal_agent_for, update_personal_agent,
};
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// `Name = z.string().trim().min(1, …).max(60, …)` — trimmed, then the two
/// named bounds.
fn parse_name(
    obj: &serde_json::Map<String, Value>,
) -> Result<Option<String>, String> {
    match obj.get("name") {
        None => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err("give it a name".into());
            }
            if trimmed.chars().count() > 60 {
                return Err("keep the name under 60 characters".into());
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

/// `Handle = z.string().trim().regex(HANDLE_RE, …)` — the note is the whole
/// message; length and alphabet both live in the regex.
fn parse_handle(
    obj: &serde_json::Map<String, Value>,
) -> Result<Option<String>, String> {
    match obj.get("handle") {
        None => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            let trimmed = s.trim();
            if !handle_ok(trimmed) {
                return Err(HANDLE_RE_NOTE.to_string());
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

/// `model: z.string().trim().min(1).max(60).optional()` — trimmed, then the
/// two default bounds.
fn parse_model(
    obj: &serde_json::Map<String, Value>,
) -> Result<Option<String>, String> {
    match obj.get("model") {
        None => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err(crate::body::too_small_msg(1));
            }
            if trimmed.chars().count() > 60 {
                return Err(crate::body::too_big_msg(60));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

/// "agent "x" already exists" (slug collision) → something a person can act on.
fn friendly(msg: &str) -> String {
    if msg.contains("already exists") {
        "that handle is taken — pick another".into()
    } else {
        msg.to_string()
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match personal_agent_for(&state.pg, &user.id).await {
        Ok(assistant) => Json(json!({ "assistant": assistant })).into_response(),
        Err(e) => {
            tracing::error!("[me.assistant] read failed: {e}");
            crate::error::thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match parse_name(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let handle = match parse_handle(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let personality = match optional_max_string_member(obj, "personality", 4000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let owner = PersonalUser {
        id: &user.id,
        email: user.email.as_deref(),
        name: user.name.as_deref(),
    };
    match create_personal_agent(
        &state,
        &owner,
        &PersonalAgentInput {
            name: name.as_deref(),
            handle: handle.as_deref(),
            personality: personality.as_deref(),
        },
    )
    .await
    {
        Ok(assistant) => Json(json!({ "assistant": assistant })).into_response(),
        Err(e) => house_error(StatusCode::BAD_REQUEST, &friendly(&e)),
    }
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match parse_name(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let handle = match parse_handle(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let personality = match optional_max_string_member(obj, "personality", 4000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let model = match parse_model(obj) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // `.refine(b => Object.values(b).some(v => v !== undefined))` — parsed
    // member keys first, then the at-least-one check.
    if name.is_none() && handle.is_none() && personality.is_none() && model.is_none() {
        return house_error(StatusCode::BAD_REQUEST, "nothing to update");
    }
    let owner = PersonalUser {
        id: &user.id,
        email: user.email.as_deref(),
        name: user.name.as_deref(),
    };
    match update_personal_agent(
        &state,
        &owner,
        &PersonalAgentPatch {
            name: name.as_deref(),
            handle: handle.as_deref(),
            personality: personality.as_deref(),
            model: model.as_deref(),
        },
    )
    .await
    {
        Ok(assistant) => Json(json!({ "assistant": assistant })).into_response(),
        Err(e) => house_error(StatusCode::BAD_REQUEST, &friendly(&e)),
    }
}
