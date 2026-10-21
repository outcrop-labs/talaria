// POST /api/agents/subagent-events. The delegation half of the talaria-events
// Hermes plugin: the harness fires subagent_start / subagent_stop around
// every child the delegate_task tool spawns, and this route lands ONE chip
// per child on the turn the parent is speaking in.
//
// WHY A CHIP RATHER THAN THE RUN WATCH. A delegating turn goes quiet. The
// child runs under its OWN session id, so the tool events the same plugin
// reports for it carry a correlation the parent's reader has no way to join,
// and the parent's stream shows a minutes-long gap that reads as a hang. The
// chip is the one surface where "it handed this out, and here is what came
// back" sits next to the sentence that caused it.
//
// Agent-authenticated, never session-authenticated — the same door as
// tool_events.rs, and the platform trusts the plugin's clamps no further
// here than it does there: kind, child id, role, goal and summary are
// re-validated, re-clamped and secret-scrubbed HERE, at the boundary.
//
// ONE CHIP, OVERWRITTEN IN PLACE. The chip id is the child's session id and
// `subagent` is a snapshot kind in talaria-chips, so a stop replaces its own
// start where it sits instead of stacking a second chip under it. A stop for
// a child whose start never arrived still lands: the start is best-effort
// observability and can be dropped under queue pressure, and a result with
// no beginning is far better than a child that silently never finished.
//
// ADDRESSED BY CONVERSATION. Both hooks carry `parentSessionId`, and for a
// pinned agent that session id IS the conversation the parent is speaking
// in; the chip lands there and nowhere else. A body without one is served
// only while exactly one turn could be meant — otherwise a 409 that says so.
//
// THE TRUST BOUNDARY IS THE AGENT, NOT THE PLUGIN: the key this door takes
// is readable from the container's own terminal, so a model can post a
// "reviewer · done — approved" chip here itself. The route re-clamps and
// scrubs, and the landing is scoped to the agent's own threads; it cannot
// tell plugin from model, and nothing downstream should treat a chip as
// proof that a child ran.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::require_agent;
use talaria_api_facades::runs::defs::work_session::scrub_secrets;
use talaria_body::{parse, truncate_utf16};
use talaria_error::{house_error, internal, object_or_400};
use talaria_state::AppState;

// doc: Report that this agent handed work to a child, and what came back.

const MAX_ID: usize = 200;
const MAX_ROLE: usize = 400;
const MAX_GOAL: usize = 2_000;
const MAX_SUMMARY: usize = 4_000;
const MAX_TOOL: usize = 120;
const MAX_TOOLS: usize = 40;
/// The goal as the chip's label carries it. The chip is one line in a
/// stream; a goal is a paragraph's worth of instruction, and the whole thing
/// rides the chip's `goal` field for a reader who opens it.
const LABEL_GOAL: usize = 80;

/// One validated event — all the chip is built from.
struct Event {
    child: String,
    role: String,
    goal: String,
    summary: String,
    status: &'static str,
    tools: Vec<String>,
}

/// A string field as it is allowed to be stored: scrubbed of credential
/// shapes, THEN clamped. That order is the point — clamping first can cut a
/// key in half, and half a key no longer matches the scrub's shapes, so the
/// prefix survives into the row.
fn field(body: &Value, key: &str, max: usize) -> String {
    let raw = body.get(key).and_then(Value::as_str).unwrap_or("").trim();
    let scrubbed = scrub_secrets(raw);
    truncate_utf16(&scrubbed, max).to_string()
}

/// `childStatus` is the harness's word, not the platform's: hermes-agent
/// says "completed" and "error" today and has said others, and a child that
/// timed out or was aborted did not succeed. Anything error-shaped is an
/// error; everything else finished.
fn stop_status(raw: &str) -> &'static str {
    let said = raw.trim().to_ascii_lowercase();
    if ["error", "fail", "timeout", "abort", "cancel"]
        .iter()
        .any(|shape| said.contains(shape))
    {
        "error"
    } else {
        "done"
    }
}

/// The tool names the child used, in order — names only, bounded. The
/// arguments and results are the child's own tool events' business.
fn tools_from(body: &Value) -> Vec<String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .take(MAX_TOOLS)
                .map(|name| truncate_utf16(name, MAX_TOOL).to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `<role> · <goal>`, with either half allowed to be missing: the harness
/// hands the hooks whatever the delegating call gave it, and a chip with an
/// empty label would read as a bug in the platform rather than a thin call.
fn label(role: &str, goal: &str) -> String {
    let goal = truncate_utf16(goal, LABEL_GOAL);
    match (role.is_empty(), goal.is_empty()) {
        (false, false) => format!("{role} · {goal}"),
        (false, true) => role.to_string(),
        (true, false) => goal.to_string(),
        (true, true) => "delegated subagent".to_string(),
    }
}

/// Validate and clamp the plugin's body, or the sentence to refuse it with.
/// Pure, so the rules are testable without a database behind them.
fn event_from(body: &Value) -> Result<Event, &'static str> {
    let stop = match body.get("kind").and_then(Value::as_str).unwrap_or("") {
        "start" => false,
        "stop" => true,
        _ => return Err("kind must be start|stop"),
    };
    // Clamped one OVER the bound and then refused: every other field is cut
    // to fit, but an id cut to fit is the id of a different child, and the
    // chip would key on it.
    let child = field(body, "childSessionId", MAX_ID + 1);
    if child.is_empty() || child.encode_utf16().count() > MAX_ID {
        return Err("childSessionId required, 1..200 characters");
    }
    Ok(Event {
        child,
        role: field(body, "childRole", MAX_ROLE),
        goal: field(body, "childGoal", MAX_GOAL),
        // A start has nothing to summarise yet, and the status it implies is
        // the only one it can have: the child is running.
        summary: if stop {
            field(body, "childSummary", MAX_SUMMARY)
        } else {
            String::new()
        },
        status: if stop {
            stop_status(
                body.get("childStatus")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            )
        } else {
            "running"
        },
        tools: if stop { tools_from(body) } else { Vec::new() },
    })
}

fn chip(event: &Event) -> Value {
    json!({
        "id": format!("subagent:{}", event.child),
        "kind": "subagent",
        "label": label(&event.role, &event.goal),
        "status": event.status,
        "goal": event.goal,
        "summary": event.summary,
        "tools": event.tools,
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    let parsed = parse(&body);
    // Shape first, so a body that is not an object at all gets the house
    // sentence rather than a sentence about one of its fields.
    object_or_400(&parsed)?;
    let event = match event_from(&parsed) {
        Ok(event) => event,
        Err(why) => return Ok(house_error(StatusCode::BAD_REQUEST, why)),
    };
    // The chip's decide gate is owner-or-admin, and tool_events.rs resolves
    // the owner the same way — passing none there made an owner-scoped chip
    // invisible to the person it was for.
    let owner =
        talaria_api_facades::google::agent::resolve_agent_principal(&state.pg, &caller.model)
            .await
            .ok()
            .and_then(|principal| principal.owner_user_id);
    let redis = state.redis().await.ok();
    let hint = talaria_chips::conversation_hint(&parsed);
    match talaria_chips::surface_tool_chips_in(
        &state.pg,
        redis,
        &caller.model,
        owner.as_deref(),
        hint.as_deref(),
        vec![chip(&event)],
    )
    .await
    {
        // Several turns and no parent session named: refused rather than
        // guessed, and the sentence says what would have resolved it.
        Ok(target @ talaria_chips::Target::Ambiguous { .. }) => Ok(house_error(
            StatusCode::CONFLICT,
            &target.ambiguity().unwrap_or_default(),
        )),
        // No live turn and nothing recent: the chip has nowhere correct to
        // land. Said on the wire rather than only in a log, because the
        // plugin's own contract is fire-and-forget and `landed` is the one
        // place this is visible at all.
        Ok(target) => {
            Ok(Json(json!({ "ok": true, "landed": target.landed().is_some() })).into_response())
        }
        Err(e) => Ok(internal("[subagent-events] chip surface failed", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The two kinds and the one required field, pinned: every other field is
    // optional because the harness fires these hooks around a call the model
    // wrote, and a thin delegation is still worth a chip.
    #[test]
    fn the_body_takes_two_kinds_and_demands_the_child_it_is_about() {
        assert!(event_from(&json!({ "kind": "bogus", "childSessionId": "c1" })).is_err());
        assert!(event_from(&json!({ "childSessionId": "c1" })).is_err());
        assert!(event_from(&json!({ "kind": "start" })).is_err());
        assert!(event_from(&json!({ "kind": "start", "childSessionId": "  " })).is_err());
        assert!(
            event_from(&json!({ "kind": "start", "childSessionId": "x".repeat(201) })).is_err()
        );
        let thin = event_from(&json!({ "kind": "start", "childSessionId": "c1" })).expect("start");
        assert_eq!(thin.status, "running");
        assert!(thin.role.is_empty() && thin.goal.is_empty());
    }

    // A start cannot carry a result, and a stop's status is the harness's
    // word mapped to the chip's two: the whole reason the mapping lives here
    // is that a container already holding the plugin cannot be re-taught.
    #[test]
    fn a_start_runs_and_a_stop_carries_the_harness_verdict() {
        let start = event_from(&json!({
            "kind": "start", "childSessionId": "c1", "childRole": "researcher",
            "childGoal": "find who owns the gate", "childSummary": "impossible",
            "childStatus": "error", "tools": ["Read"],
        }))
        .expect("start");
        assert_eq!(start.status, "running");
        assert_eq!(start.summary, "", "a start has nothing to summarise yet");
        assert!(start.tools.is_empty());

        let done = event_from(&json!({
            "kind": "stop", "childSessionId": "c1", "childStatus": "completed",
            "childSummary": "the owner is Dana",
        }))
        .expect("stop");
        assert_eq!(done.status, "done");
        assert_eq!(done.summary, "the owner is Dana");

        for said in [
            "error",
            "ERROR",
            "failed",
            "timeout",
            "aborted",
            "cancelled",
        ] {
            assert_eq!(stop_status(said), "error", "{said}");
        }
        for said in ["completed", "done", "ok", ""] {
            assert_eq!(stop_status(said), "done", "{said}");
        }
    }

    // One line in a stream. The goal is clipped hard and the whole goal rides
    // the chip's own field — and a key pasted into a goal never reaches the
    // row, which is why the scrub runs before the clamp.
    #[test]
    fn the_label_is_the_role_and_a_clipped_goal() {
        assert_eq!(
            label("researcher", "find the owner"),
            "researcher · find the owner"
        );
        assert_eq!(label("researcher", ""), "researcher");
        assert_eq!(label("", "find the owner"), "find the owner");
        assert_eq!(label("", ""), "delegated subagent");
        let long = label("r", &"g".repeat(200));
        assert_eq!(long.chars().count(), "r · ".chars().count() + LABEL_GOAL);

        let event = event_from(&json!({
            "kind": "stop", "childSessionId": "c1", "childRole": "coder",
            "childGoal": "use tak_abcdefghijklmnop to call the api",
            "childStatus": "completed",
            "tools": ["Read", "", "Edit"],
        }))
        .expect("stop");
        assert!(!event.goal.contains("tak_abcdefgh"), "{}", event.goal);
        assert!(event.goal.contains("[redacted:key]"));
        assert_eq!(event.tools, ["Read", "Edit"]);
        let chip = chip(&event);
        assert_eq!(chip["id"], "subagent:c1");
        assert_eq!(chip["kind"], "subagent");
        assert_eq!(chip["status"], "done");
    }
}
