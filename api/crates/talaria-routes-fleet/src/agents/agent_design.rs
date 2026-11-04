// doc: The caller's agent design — New agent's "describe it", owned by the
// doc: server (talaria-agent-design). The record lives in Redis for 72 hours;
// doc: the muse call is an `agent-design` run, so a design survives a reload,
// doc: a closed tab and a restart. One design per person.
//
// /api/fleet/agent-design.
//   GET    → `{ design, now }`: the design or null, and the server's clock so
//            the elapsed count agrees across tabs. `?watching=1` is the open
//            modal: a landing in the next few seconds skips the bell.
//   POST   → start (`{ instruction }`) or refine (`{ instruction, refine:
//            true, current? }`). A start while one generates answers that one.
//   PATCH  → the person's own draft (review-step edits, a role template, a
//            manual start). 409 while a run is writing it.
//   DELETE → discard (`{ id? }`); 404 when there is nothing (another tab got
//            there first). A hire with `designId` consumes it too.
// The permission is the hire's (`agents.manage`): the design exists to be
// hired.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_design::{
    AgentDesign, DesignError, DraftFields, SkillDraft, current_design, discard_design, edit_design,
    refine_design, start_design,
};
use talaria_body::{
    array_msg, array_too_big_msg, object_msg, optional_boolean_member, optional_uuid_member, parse,
    string_member, zod_type_name,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_perm;
use talaria_state::AppState;

#[derive(serde::Deserialize)]
pub struct WatchQuery {
    watching: Option<String>,
}

fn answer(design: Option<AgentDesign>) -> Response {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Json(json!({ "design": design, "now": now })).into_response()
}

fn failed(e: DesignError) -> Response {
    match e {
        DesignError::Refused(r) => house_error(StatusCode::CONFLICT, r.sentence()),
        DesignError::Fault(e) => internal("[agent-design] failed", e),
    }
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<WatchQuery>,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    let watching = matches!(q.watching.as_deref(), Some("1" | "true"));
    Ok(match current_design(&state, &user.id, watching).await {
        Ok(d) => answer(d),
        Err(e) => failed(e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let instruction = match string_member(obj, "instruction", 1, 8_000) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        Ok(_) => {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                &talaria_body::too_small_msg(1),
            ));
        }
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let refine = match optional_boolean_member(obj, "refine") {
        Ok(v) => v.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let current = match obj.get("current") {
        None | Some(Value::Null) => None,
        Some(v) => match v
            .as_object()
            .ok_or_else(|| object_msg(zod_type_name(v)))
            .and_then(draft_fields)
        {
            Ok(d) => Some(d),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
    };
    // The ledger's caller, as the muse route stamps it: email, else name,
    // else the id.
    let caller = format!(
        "platform:muse:{}",
        user.email
            .as_deref()
            .or(user.name.as_deref())
            .unwrap_or(&user.id)
    );
    let out = if refine {
        refine_design(&state, &user.id, &caller, &instruction, current).await
    } else {
        start_design(&state, &user.id, &caller, &instruction).await
    };
    Ok(match out {
        Ok(d) => Json(json!({ "design": d })).into_response(),
        Err(e) => failed(e),
    })
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let fields = match draft_fields(obj) {
        Ok(f) => f,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(match edit_design(&state, &user.id, fields).await {
        Ok(d) => Json(json!({ "design": d })).into_response(),
        Err(e) => failed(e),
    })
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_perm(&state, &headers, "agents.manage").await?;
    // An empty body is a discard of whatever the caller holds.
    let parsed = if body.is_empty() {
        json!({})
    } else {
        parse(&body)
    };
    let obj = object_or_400(&parsed)?;
    let id = match optional_uuid_member(obj, "id") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
        match discard_design(&state, &user.id, id.as_deref()).await {
            Ok(true) => Json(json!({ "ok": true })).into_response(),
            Ok(false) => house_error(StatusCode::NOT_FOUND, "not found"),
            Err(e) => failed(e),
        },
    )
}

/// The draft as the review step holds it. Bounds are the hire's own (so a
/// saved draft is one the hire accepts), with the empty string legal: a
/// manual start fills its fields one keystroke at a time.
fn draft_fields(obj: &serde_json::Map<String, Value>) -> Result<DraftFields, String> {
    let skills = match obj.get("skills") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => {
            let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
            if arr.len() > 5 {
                return Err(array_too_big_msg(5));
            }
            let mut out = Vec::with_capacity(arr.len());
            for el in arr {
                let t = el
                    .as_object()
                    .ok_or_else(|| object_msg(zod_type_name(el)))?;
                out.push(SkillDraft {
                    name: string_member(t, "name", 0, 200)?,
                    content: string_member(t, "content", 0, 100_000)?,
                });
            }
            out
        }
    };
    Ok(DraftFields {
        name: string_member(obj, "name", 0, 60)?,
        handle: string_member(obj, "handle", 0, 30)?,
        department: string_member(obj, "department", 0, 40)?,
        role: string_member(obj, "role", 0, 80)?,
        soul: string_member(obj, "soul", 0, 200_000)?,
        skills,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_reads_with_empty_fields_and_the_hires_bounds() {
        let obj = json!({ "name": "", "handle": "", "department": "", "role": "", "soul": "" });
        let d = draft_fields(obj.as_object().unwrap()).unwrap();
        assert_eq!(d, DraftFields::default());
        let long = json!({ "name": "x".repeat(61), "handle": "", "department": "", "role": "", "soul": "" });
        assert!(draft_fields(long.as_object().unwrap()).is_err());
        let six = json!({
            "name": "a", "handle": "a", "department": "a", "role": "a", "soul": "a",
            "skills": (0..6).map(|i| json!({ "name": format!("s{i}"), "content": "" })).collect::<Vec<_>>(),
        });
        assert!(draft_fields(six.as_object().unwrap()).is_err());
        let missing = json!({ "name": "a" });
        assert!(draft_fields(missing.as_object().unwrap()).is_err());
    }
}
