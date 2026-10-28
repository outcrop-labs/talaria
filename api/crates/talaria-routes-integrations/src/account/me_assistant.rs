// /api/me/assistant. The signed-in user's personal assistant. GET → theirs
// (or null), with personality + live status. POST → create + start one,
// optionally named/personalized (idempotent: returns the existing one,
// re-enabling if retired). PATCH → owner-scoped rename / personality edit,
// and `dmAccess` — how far the assistant may reach into the owner's own
// human↔human direct messages (`none` | `read` | `read-write`). Any signed-in
// user; every operation is keyed on agent_defs.owner_user_id, never on a
// client-sent id.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_body::{optional_max_string_member, parse, string_msg, zod_type_name};
use talaria_error::{house_error, internal, object_or_400};
use talaria_personal_agent::{
    ASSISTANT_DISPLACED, HANDLE_RE_NOTE, PersonalAgent, PersonalAgentInput, PersonalAgentPatch,
    PersonalUser, create_personal_agent, handle_ok, is_department_taken, personal_agent_for,
    update_personal_agent,
};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_users::{
    ASSISTANT_DM_ACCESS_OPTIONS, AssistantDmAccess, assistant_dm_access, set_assistant_dm_access,
};

/// name — trimmed first, then the two named bounds: empty → "give it a
/// name", over 60 chars → "keep the name under 60 characters".
fn parse_name(obj: &serde_json::Map<String, Value>) -> Result<Option<String>, String> {
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

/// handle — trimmed, then checked against HANDLE_RE; the note is the whole
/// message (length and alphabet both live in the regex).
fn parse_handle(obj: &serde_json::Map<String, Value>) -> Result<Option<String>, String> {
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

/// model — trimmed, then the two default bounds (min 1, max 60).
fn parse_model(obj: &serde_json::Map<String, Value>) -> Result<Option<String>, String> {
    match obj.get("model") {
        None => Ok(None),
        Some(v) => {
            let s = v.as_str().ok_or_else(|| string_msg(zod_type_name(v)))?;
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Err(talaria_body::too_small_msg(1));
            }
            if trimmed.chars().count() > 60 {
                return Err(talaria_body::too_big_msg(60));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

/// dmAccess — one of the three reaches, or absent. The enum message names
/// the three.
fn parse_dm_access(
    obj: &serde_json::Map<String, Value>,
) -> Result<Option<AssistantDmAccess>, String> {
    Ok(
        talaria_body::optional_enum_member(obj, "dmAccess", &ASSISTANT_DM_ACCESS_OPTIONS)?
            .as_deref()
            .and_then(AssistantDmAccess::parse),
    )
}

/// The wire answer: the assistant (or null) with the owner's `dmAccess`
/// beside its other preferences. The setting is the owner's, not the
/// agent row's — it is read here so a recreated assistant keeps it — and it
/// rides inside the object because that is where every other knob the
/// Settings → Assistant card shows already lives.
async fn answer(state: &AppState, user_id: &str, assistant: Option<PersonalAgent>) -> Response {
    let assistant = match assistant {
        None => Value::Null,
        Some(a) => {
            let mut v = serde_json::to_value(a).unwrap_or(Value::Null);
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "dmAccess".into(),
                    Value::String(
                        assistant_dm_access(&state.pg, user_id)
                            .await
                            .as_str()
                            .into(),
                    ),
                );
            }
            v
        }
    };
    Json(json!({ "assistant": assistant })).into_response()
}

/// "agent "x" already exists" (slug collision) → something a person can act on.
fn friendly(msg: &str) -> String {
    if msg.contains("already exists") {
        "that handle is taken — pick another".into()
    } else {
        msg.to_string()
    }
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match personal_agent_for(&state.pg, &user.id).await {
        Ok(assistant) => answer(&state, &user.id, assistant).await,
        Err(e) => internal("[me.assistant] read failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match parse_name(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let handle = match parse_handle(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let personality = match optional_max_string_member(obj, "personality", 4000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let owner = PersonalUser {
        id: &user.id,
        email: user.email.as_deref(),
        name: user.name.as_deref(),
    };
    Ok(
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
            Ok(assistant) => answer(&state, &user.id, Some(assistant)).await,
            // The handle names the assistant's department, and another agent
            // already holds it — the same conflict a hire meets, said without
            // naming somebody else's agent.
            Err(e) if is_department_taken(&e) => house_error(
                StatusCode::CONFLICT,
                "that handle is taken — another agent already runs under it; pick another",
            ),
            // A retired assistant whose department another agent shares and
            // runs in (a pair from before the one-agent rule) stays retired.
            Err(e) if e == ASSISTANT_DISPLACED => house_error(StatusCode::CONFLICT, &e),
            Err(e) => house_error(StatusCode::BAD_REQUEST, &friendly(&e)),
        },
    )
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match parse_name(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let handle = match parse_handle(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let personality = match optional_max_string_member(obj, "personality", 4000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let model = match parse_model(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let dm_access = match parse_dm_access(obj) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // members parse first (each gets its own error), then the at-least-one
    // check rejects an empty patch.
    if name.is_none()
        && handle.is_none()
        && personality.is_none()
        && model.is_none()
        && dm_access.is_none()
    {
        return Ok(house_error(StatusCode::BAD_REQUEST, "nothing to update"));
    }
    let owner = PersonalUser {
        id: &user.id,
        email: user.email.as_deref(),
        name: user.name.as_deref(),
    };
    // The DM reach is the owner's setting, not a version of the agent: it
    // lands without a new soul/config version and without a restart, and a
    // patch that carries only it never touches the agent at all.
    if let Some(access) = dm_access
        && let Err(e) = set_assistant_dm_access(&state.pg, &user.id, access).await
    {
        return Ok(internal("[me.assistant] dm access write failed", e));
    }
    if name.is_none() && handle.is_none() && personality.is_none() && model.is_none() {
        return Ok(match personal_agent_for(&state.pg, &user.id).await {
            Ok(assistant) => answer(&state, &user.id, assistant).await,
            Err(e) => internal("[me.assistant] read failed", e),
        });
    }
    Ok(
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
            Ok(assistant) => answer(&state, &user.id, Some(assistant)).await,
            Err(e) => house_error(StatusCode::BAD_REQUEST, &friendly(&e)),
        },
    )
}
