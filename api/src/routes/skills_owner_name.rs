// /api/skills/{owner}/{name} — port of ui/src/routes/api/skills.$owner.$name.ts.
// One skill's SKILL.md. GET → content + file list (any member — the library
// is org work material). PUT → save (creates the skill if new). DELETE →
// remove the whole skill dir. Writes go through canEditSkill(s): admin /
// agents.manage everywhere; personal-assistant owners and explicit
// user_agent_access grantees for that agent's own skills. Edits are LIVE
// (Hermes reads skills per invocation, no restart).

use crate::agent_skills::{copy_skill, delete_skill, read_skill, rename_skill, write_skill};
use crate::body::{as_object, parse, string_member, too_big_msg, too_small_msg};
use crate::error::{house_error, thrown_internal_error};
use crate::session::require_user;
use crate::skill_access::{can_edit_skill, can_edit_skills};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The union flattens every structural failure into zod's bare
/// `Invalid input` — a non-string, a missing key, an op no branch carries.
const UNION_MSG: &str = "Invalid input";
/// zod's default regex sentence for `NAME` (`/^[a-z0-9][a-z0-9._-]*$/`).
const NAME_MSG: &str = "Invalid string: must match pattern /^[a-z0-9][a-z0-9._-]*$/";

fn name_pattern_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'))
}

/// `NAME = z.string().regex(...).max(80)` as an optional member: absent →
/// None; present → string, then the regex, then the max (regex first — a
/// long name of bad characters answers the regex sentence).
fn check_name(value: Option<&serde_json::Value>) -> Result<Option<String>, String> {
    let Some(v) = value else { return Ok(None) };
    let s = v.as_str().ok_or_else(|| UNION_MSG.to_string())?;
    if !name_pattern_ok(s) {
        return Err(NAME_MSG.to_string());
    }
    if s.len() > 80 {
        return Err(too_big_msg(80));
    }
    Ok(Some(s.to_string()))
}

enum Op {
    Rename { to_name: String },
    CopyMove {
        to_owner: String,
        to_name: Option<String>,
        remove_source: bool,
    },
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Response {
    if let Err(gate) = require_user(&state, &headers).await {
        return gate;
    }
    match read_skill(&state.pg, &owner, &name).await {
        Ok((content, files)) => Json(json!({ "content": content, "files": files })).into_response(),
        Err(e) => house_error(StatusCode::NOT_FOUND, &e),
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let gate = can_edit_skill(&state.pg, &user.id, &user.role, &owner, &name).await;
    match gate {
        Ok(true) => {}
        Ok(false) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[skills] edit gate failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.string().max(500_000) — required, and the empty string is legal.
    let content = match string_member(obj, "content", 0, 500_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let author = user.email.as_deref().or(user.name.as_deref()).unwrap_or("admin");
    match write_skill(&state.pg, &owner, &name, &content, Some(author)).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}

/// Structural ops: rename in place, copy/move to another owner (e.g. promote
/// an agent's skill to shared). Copy needs write on the DESTINATION;
/// rename/move also on the source.
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
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
    let op = match obj.get("op") {
        Some(serde_json::Value::String(s)) if s == "rename" || s == "copy" || s == "move" => {
            s.as_str()
        }
        _ => return house_error(StatusCode::BAD_REQUEST, UNION_MSG),
    };
    let op = if op == "rename" {
        let to_name = match check_name(obj.get("toName")) {
            Ok(Some(v)) => v,
            Ok(None) => return house_error(StatusCode::BAD_REQUEST, UNION_MSG),
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        Op::Rename { to_name }
    } else {
        // copy | move — toOwner is required, 1..80
        let to_owner = match obj.get("toOwner") {
            None | Some(serde_json::Value::Null) => {
                return house_error(StatusCode::BAD_REQUEST, UNION_MSG)
            }
            Some(serde_json::Value::String(s)) => {
                if s.is_empty() {
                    return house_error(StatusCode::BAD_REQUEST, &too_small_msg(1));
                }
                if s.chars().count() > 80 {
                    return house_error(StatusCode::BAD_REQUEST, &too_big_msg(80));
                }
                s.clone()
            }
            Some(_) => return house_error(StatusCode::BAD_REQUEST, UNION_MSG),
        };
        let to_name = match check_name(obj.get("toName")) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };
        Op::CopyMove {
            to_owner,
            to_name,
            remove_source: op == "move",
        }
    };
    let need_source = !matches!(op, Op::Rename { .. });
    if need_source {
        match can_edit_skill(&state.pg, &user.id, &user.role, &owner, &name).await {
            Ok(true) => {}
            Ok(false) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
            Err(e) => {
                tracing::error!("[skills] source gate failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    let dest = match &op {
        Op::Rename { .. } => owner.as_str(),
        Op::CopyMove { to_owner, .. } => to_owner.as_str(),
    };
    match can_edit_skills(&state.pg, &user.id, &user.role, dest).await {
        Ok(true) => {}
        Ok(false) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[skills] destination gate failed: {e}");
            return thrown_internal_error();
        }
    }
    let outcome = match op {
        Op::Rename { to_name } => rename_skill(&state.pg, &owner, &name, &to_name).await,
        Op::CopyMove {
            to_owner,
            to_name,
            remove_source,
        } => {
            copy_skill(&state.pg, &owner, &name, &to_owner, to_name.as_deref(), remove_source)
                .await
        }
    };
    match outcome {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match can_edit_skill(&state.pg, &user.id, &user.role, &owner, &name).await {
        Ok(true) => {}
        Ok(false) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[skills] edit gate failed: {e}");
            return thrown_internal_error();
        }
    }
    match delete_skill(&state.pg, &owner, &name).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}
