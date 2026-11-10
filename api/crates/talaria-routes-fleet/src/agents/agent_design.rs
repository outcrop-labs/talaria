// doc: The caller's agent design — New agent's "describe it", owned by the
// doc: server (talaria-agent-design). The record lives in Redis for 72 hours;
// doc: the muse call is an `agent-design` run, so a design survives a reload,
// doc: a closed tab and a restart. One design per person.
//
// /api/fleet/agent-design.
//   GET    → `{ design, hiring, returned, now }`: the design or null, the
//            designs the person's hires are spending, any a failed hire handed
//            back while the slot was taken, and the server's clock so the
//            elapsed count agrees across tabs. `?watching=1` is the open
//            modal: a landing in the next few seconds skips the bell.
//   POST   → start (`{ instruction }`) or refine (`{ instruction, refine:
//            true, id, current? }`). A start while one generates answers that
//            one.
//   PATCH  → the person's own draft (review-step edits, a role template, a
//            manual start), naming the design (`id`) unless there is none.
//            409 while a run or a hire holds it, or when `id` is stale.
//            A start, or a PATCH with no `id`, over a design a failed hire
//            handed back that nobody has edited or refined since sets that
//            design aside: it waits in `returned` until the new one is hired
//            or discarded, instead of being replaced or refusing the start.
//   DELETE → discard (`{ id? }`); 404 when there is nothing (another tab got
//            there first). A hire with `designId` claims it, and the design is
//            consumed once that hire finishes. The record's `chat` stays on
//            the server; the wire carries `chatLength`.
// The permission is the hire's (`agents.manage`): the design exists to be
// hired.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_design::{
    AgentDesign, DesignError, DesignView, DraftFields, SkillDraft, current_design, discard_design,
    edit_design, refine_design, start_design,
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

/// The record on the wire. The chat stays on the server: it is up to ten
/// turns of whole drafts (a soul alone may be 200k characters), the open
/// modal reads every two seconds while a design generates, and the UI needs
/// only to know whether there is one (`chatLength`).
fn wire(d: &AgentDesign) -> Value {
    let mut v = serde_json::to_value(d).expect("a design serializes");
    if let Some(o) = v.as_object_mut() {
        o.remove("chat");
        o.insert("chatLength".into(), json!(d.chat.len()));
    }
    v
}

fn answer(view: DesignView) -> Response {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Json(json!({
        "design": view.design.as_ref().map(wire),
        "hiring": view.hiring,
        "returned": view.returned,
        "now": now,
    }))
    .into_response()
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
    // The design a refine revises; a start names none.
    let id = match optional_uuid_member(obj, "id") {
        Ok(v) => v,
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
        refine_design(
            &state,
            &user.id,
            &caller,
            id.as_deref(),
            &instruction,
            current,
        )
        .await
    } else {
        start_design(&state, &user.id, &caller, &instruction).await
    };
    Ok(match out {
        Ok(d) => Json(json!({ "design": wire(&d) })).into_response(),
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
    // The design these edits are for: absent only for a manual start, where
    // the person holds none. Naming another is the stale tab's 409.
    let id = match optional_uuid_member(obj, "id") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
        match edit_design(&state, &user.id, id.as_deref(), fields).await {
            Ok(d) => Json(json!({ "design": wire(&d) })).into_response(),
            Err(e) => failed(e),
        },
    )
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

    /// The routes' code, comments dropped, as one string: the wiring tests
    /// below read what the handlers call, not what their comments say.
    fn code(src: &str) -> String {
        src.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_hire_route_goes_through_the_design_gate() {
        // `gate_hire` pins claim → enqueue → release; this pins that the
        // hire route uses it, and enqueues nowhere else.
        let c = code(include_str!("../fleet/fleet_create.rs"));
        assert!(
            c.contains("talaria_agent_design::claim_for_hire("),
            "the hire claims the design"
        );
        assert!(
            c.contains("talaria_agent_design::gate_hire(claim, enqueued, release)"),
            "the hire goes through gate_hire"
        );
        assert_eq!(
            c.matches("enqueue(").count(),
            1,
            "the only enqueue is the one handed to the gate"
        );
        assert!(c.contains("Box::pin(enqueue("));
        assert!(
            !c.contains("consume_design"),
            "the design is never consumed after the enqueue"
        );
    }

    #[test]
    fn the_retry_route_takes_the_design_along_before_it_enqueues() {
        let c = code(include_str!("../fleet/fleet_hires_id_retry.rs"));
        let follow = c
            .find("talaria_agent_design::follow_retry(")
            .expect("the retry follows the design");
        let enqueue = c
            .find("let row = match enqueue(")
            .expect("the retry enqueues");
        assert!(
            follow < enqueue,
            "the design follows the retry before the retry can finish"
        );
        assert!(
            c.contains(".get(\"managerUserId\")"),
            "the design is the hirer's, not the retrier's"
        );
    }
}
