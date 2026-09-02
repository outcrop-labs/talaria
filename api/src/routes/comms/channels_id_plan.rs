// /api/channels/{id}/plan.
// The channel Plan button: POST enqueues a 'plan-draft' run on a channel
// agent and answers immediately with the queued draft; GET/PATCH/DELETE
// read, persist edits to, and drop the channel's latest draft. Members only;
// the drafting agent must be in the channel and usable by the caller.
// Nothing is created here — the human reviews and creates via the boards API.
//
// The two plan-draft routes (this one and plans_id_draft.rs) share one
// domain module (plan_drafts.rs + the plan-draft run def), and the Start
// and Save validators are said once, here. The one place the pair differs
// — channels names the drafting agent in the body; the plan surface reads
// it from the conversation — is the `with_agent` switch, the same shape
// workflows' create/patch pair uses.

use crate::body::{
    array_msg, array_too_big_msg, as_object, boolean_member, enum_member, enum_msg, object_msg,
    optional_max_string_member, optional_uuid_member, string_member, too_big_msg, utf16_len,
    zod_type_name,
};
use crate::channels::{channel_role, list_channel_agents};
use crate::error::{house_error, thrown_internal_error};
use crate::fleet::{routed_model_for, usable_agent_gate};
use crate::harness::defs::channel_plan::{Effort, Priority};
use crate::plan_drafts::{
    StartPlanDraft, drop_draft, latest_draft_for, save_draft_proposals, start_plan_draft,
};
use crate::runs::defs::plan_draft::StoredProposal;
use crate::session::require_user;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

const PRIORITIES: &[&str] = &["low", "medium", "high", "urgent"];
const EFFORTS: &[&str] = &["xs", "s", "m", "l", "xl"];

/// The Start body: the channel side names the drafting agent; the plan
/// side inherits it from the conversation.
#[derive(Debug)]
pub(crate) struct StartBody {
    pub agent_model: Option<String>,
    /// tier: max 60, nullish — null, absent AND '' all mean "no tier
    /// picked" (the routing ask below skips an empty tier); '' still stays
    /// '' in the stored row.
    pub tier: Option<String>,
    pub board_id: Option<String>,
    pub template_id: Option<String>,
}

pub(crate) fn validate_start(
    obj: &serde_json::Map<String, Value>,
    with_agent: bool,
) -> Result<StartBody, String> {
    let agent_model = if with_agent {
        Some(string_member(obj, "agentModel", 1, 200)?)
    } else {
        None
    };
    let tier = nullish_max_string_member(obj, "tier", 60)?;
    let board_id = optional_uuid_member(obj, "boardId")?;
    let template_id = optional_uuid_member(obj, "templateId")?;
    Ok(StartBody {
        agent_model,
        tier,
        board_id,
        template_id,
    })
}

/// Nullish string member: absent and null both pass as None; a present
/// value is a string within bounds (max n), the empty string included.
fn nullish_max_string_member(
    obj: &serde_json::Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => optional_max_string_member(obj, key, max),
    }
}

/// The Save body: the review walk's writes, the full batch already
/// normalized — what the run produced plus the human's edits, arriving as
/// Vec<StoredProposal> exactly as the row stores it. Fields checked in
/// schema order, elements in array order, so the first bad field of the
/// first bad element is the error that answers.
pub(crate) fn validate_save_body(
    obj: &serde_json::Map<String, Value>,
) -> Result<Vec<StoredProposal>, String> {
    let v = obj.get("proposals").ok_or_else(|| array_msg("undefined"))?;
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    if arr.len() > 50 {
        return Err(array_too_big_msg(50));
    }
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let e = item
            .as_object()
            .ok_or_else(|| object_msg(zod_type_name(item)))?;
        let title = string_member(e, "title", 0, 500)?;
        let description = string_member(e, "description", 0, 20_000)?;
        let priority = priority_of(&enum_member(e, "priority", PRIORITIES)?)?;
        let effort = match e.get("effort") {
            // effort: nullable but required — the key must be present, and
            // null is the model's "did not hazard one".
            None => return Err(enum_msg(EFFORTS)),
            Some(Value::Null) => None,
            Some(_) => Some(effort_of(&enum_member(e, "effort", EFFORTS)?)?),
        };
        let depends_on = depends_on_member(e)?;
        let tags = tags_member(e)?;
        let include = boolean_member(e, "include")?;
        out.push(StoredProposal {
            title,
            description,
            priority,
            effort,
            depends_on,
            tags,
            include,
        });
    }
    Ok(out)
}

/// dependsOn: required, unbounded array of ints — indices into the batch.
/// The bounds are the safe-integer ones (the same two-sided messages
/// body.rs spells elsewhere); the >=0 refusal is the one narrowing beyond
/// them — a negative "index" cannot name a proposal, and the only writer
/// of this body is our own review walk, which copies the run's output.
fn depends_on_member(e: &serde_json::Map<String, Value>) -> Result<Vec<usize>, String> {
    let v = e.get("dependsOn").ok_or_else(|| array_msg("undefined"))?;
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let n = el.as_f64().ok_or_else(|| {
            format!(
                "Invalid input: expected number, received {}",
                zod_type_name(el)
            )
        })?;
        if n.fract() != 0.0 {
            return Err("Invalid input: expected int, received number".into());
        }
        if n > 9_007_199_254_740_991.0 {
            return Err("Too big: expected int to be <=9007199254740991".into());
        }
        if n < -9_007_199_254_740_991.0 {
            return Err("Too small: expected int to be >=-9007199254740991".into());
        }
        out.push(
            usize::try_from(n as i64)
                .map_err(|_| "Too small: expected int to be >=0".to_string())?,
        );
    }
    Ok(out)
}

/// tags: required, unbounded array of strings (max 100 each) — free-form
/// labels.
fn tags_member(e: &serde_json::Map<String, Value>) -> Result<Vec<String>, String> {
    let v = e.get("tags").ok_or_else(|| array_msg("undefined"))?;
    let arr = v.as_array().ok_or_else(|| array_msg(zod_type_name(v)))?;
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let s = el.as_str().ok_or_else(|| {
            format!(
                "Invalid input: expected string, received {}",
                zod_type_name(el)
            )
        })?;
        if utf16_len(s) > 100 {
            return Err(too_big_msg(100));
        }
        out.push(s.to_string());
    }
    Ok(out)
}

/// enum_member has already pinned the spelling; this re-parses the same
/// string into the typed field the row stores (serde's lowercase rename
/// matches the wire spelling).
fn priority_of(s: &str) -> Result<Priority, String> {
    serde_json::from_value(json!(s)).map_err(|_| enum_msg(PRIORITIES))
}

fn effort_of(s: &str) -> Result<Effort, String> {
    serde_json::from_value(json!(s)).map_err(|_| enum_msg(EFFORTS))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on GET plan failed: {e}");
            return thrown_internal_error();
        }
    }
    match latest_draft_for(&state.pg, &id).await {
        Ok(draft) => Json(json!({ "draft": draft })).into_response(),
        Err(e) => {
            tracing::error!("[channels] draft read failed: {e}");
            thrown_internal_error()
        }
    }
}

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
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on POST plan failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let start = match validate_start(obj, true) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // validate_start(_, true) always sets it; unwrap_or_default only keeps
    // the compiler from proving what the boolean switch already guarantees.
    let agent_model = start.agent_model.clone().unwrap_or_default();

    let agents = match list_channel_agents(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[channels] agent list read on POST plan failed: {e}");
            return thrown_internal_error();
        }
    };
    if !agents.iter().any(|a| a == &agent_model) {
        return house_error(StatusCode::BAD_REQUEST, "that agent is not in this channel");
    }
    let gate = match usable_agent_gate(&state.pg, &user.id, &user.role).await {
        Ok(g) => g,
        Err(e) => {
            tracing::error!("[channels] agent access read on POST plan failed: {e}");
            return thrown_internal_error();
        }
    };
    if !gate(&agent_model) {
        return house_error(
            StatusCode::FORBIDDEN,
            "you do not have access to that agent",
        );
    }
    // Tier routing: a blank tier never asks; a failed read or an unknown
    // tier falls back to the base agent — never a 500.
    let routed = match start.tier.as_deref() {
        Some(t) if !t.is_empty() => routed_model_for(&state.pg, &agent_model, Some(t))
            .await
            .ok()
            .flatten(),
        _ => None,
    }
    .unwrap_or_else(|| agent_model.clone());

    match start_plan_draft(
        &state,
        StartPlanDraft {
            conversation_id: &id,
            source: "channel",
            user_id: &user.id,
            agent_model: &agent_model,
            routed_model: &routed,
            tier: start.tier.as_deref(),
            board_id: start.board_id.as_deref(),
            template_id: start.template_id.as_deref(),
        },
    )
    .await
    {
        Ok(draft) => Json(json!({ "draft": draft })).into_response(),
        Err(e) => {
            // Row-creation failures are internal text (docker, pg); the run's
            // OWN failures reach the client through the draft row's `error`
            // field, not this 500 — so the body is a fixed sentence.
            tracing::error!(
                "[channels] start plan draft failed for channel {id} agent {agent_model}: {e}"
            );
            house_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not start the plan draft — see server logs",
            )
        }
    }
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on PATCH plan failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let proposals = match validate_save_body(obj) {
        Ok(p) => p,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = save_draft_proposals(&state.pg, &id, &proposals).await {
        tracing::error!("[channels] save draft proposals failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::FORBIDDEN, "forbidden"),
        Err(e) => {
            tracing::error!("[channels] role read on DELETE plan failed: {e}");
            return thrown_internal_error();
        }
    }
    if let Err(e) = drop_draft(&state, &id).await {
        tracing::error!("[channels] drop draft failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn save_body(n: usize) -> Value {
        json!({
            "proposals": (0..n).map(|i| json!({
                "title": format!("ticket {i}"),
                "description": "the work",
                "priority": "medium",
                "effort": if i % 2 == 0 { json!("m") } else { json!(null) },
                "dependsOn": if i > 0 { json!([0]) } else { json!([]) },
                "tags": ["billing"],
                "include": true,
            })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn the_walk_round_trips() {
        let body = save_body(2);
        let proposals = validate_save_body(body.as_object().unwrap()).unwrap();
        assert_eq!(proposals.len(), 2);
        assert_eq!(proposals[1].priority, Priority::Medium);
        assert_eq!(proposals[0].effort, Some(Effort::M));
        assert_eq!(proposals[1].effort, None);
        assert_eq!(proposals[1].depends_on, vec![0]);
        assert_eq!(proposals[1].tags, vec!["billing".to_string()]);
        assert!(proposals[1].include);
    }

    #[test]
    fn over_fifty_is_the_arrays_own_message() {
        let body = save_body(51);
        assert_eq!(
            validate_save_body(body.as_object().unwrap()).unwrap_err(),
            array_too_big_msg(50)
        );
    }

    #[test]
    fn field_errors_speak_zod_in_schema_order() {
        let mut body = save_body(1);
        body["proposals"][0]["priority"] = json!("whenever");
        assert_eq!(
            validate_save_body(body.as_object().unwrap()).unwrap_err(),
            enum_msg(PRIORITIES)
        );
        let mut body = save_body(1);
        body["proposals"][0]
            .as_object_mut()
            .unwrap()
            .remove("include");
        assert_eq!(
            validate_save_body(body.as_object().unwrap()).unwrap_err(),
            "Invalid input: expected boolean, received undefined"
        );
        let mut body = save_body(1);
        body["proposals"][0]
            .as_object_mut()
            .unwrap()
            .remove("effort");
        assert_eq!(
            validate_save_body(body.as_object().unwrap()).unwrap_err(),
            enum_msg(EFFORTS)
        );
    }

    #[test]
    fn depends_on_is_ints_and_only_ints() {
        let mut body = save_body(1);
        body["proposals"][0]["dependsOn"] = json!(["0"]);
        assert_eq!(
            validate_save_body(body.as_object().unwrap()).unwrap_err(),
            "Invalid input: expected number, received string"
        );
        let mut body = save_body(1);
        body["proposals"][0]["dependsOn"] = json!([0.5]);
        assert_eq!(
            validate_save_body(body.as_object().unwrap()).unwrap_err(),
            "Invalid input: expected int, received number"
        );
        // 2.0 carries no fraction — it IS the integer 2.
        let mut body = save_body(1);
        body["proposals"][0]["dependsOn"] = json!([2.0]);
        assert_eq!(
            validate_save_body(body.as_object().unwrap()).unwrap()[0].depends_on,
            vec![2]
        );
    }

    #[test]
    fn the_start_body_takes_the_tier_three_ways() {
        let with_agent = |v: Value| {
            let mut v = v;
            v["agentModel"] = json!("engineer");
            validate_start(v.as_object().unwrap(), true).unwrap()
        };
        assert_eq!(with_agent(json!({})).tier, None);
        assert_eq!(with_agent(json!({ "tier": null })).tier, None);
        assert_eq!(with_agent(json!({ "tier": "" })).tier, Some(String::new()));
        assert_eq!(
            with_agent(json!({ "tier": "opus" })).tier.as_deref(),
            Some("opus")
        );
        // The plan side has no agentModel to read — absent is None, present
        // still validates: min 1 on the channel route, uuid on both.
        assert_eq!(
            validate_start(json!({}).as_object().unwrap(), false)
                .unwrap()
                .agent_model,
            None
        );
        let bad = json!({ "agentModel": "", "tier": null });
        assert_eq!(
            validate_start(bad.as_object().unwrap(), true).unwrap_err(),
            "Too small: expected string to have >=1 characters"
        );
    }
}
