// POST /api/agents/affinity. The metering half of the talaria-events Hermes
// plugin: which conversation the affinity key Hermes stamps on this agent's
// LLM calls belongs to.
//
// The relay books every call an agent makes, and Hermes names the
// conversation on each one in `x-talaria-session` (the rendered provider
// entry's `session_affinity_header`). On a turn Talaria drives, that name is
// Hermes' own digest of the session's route (`gwk_…`), which only Hermes can
// compute — so the plugin reads it at the start of every turn, off the same
// function that builds the header, and posts it here with the conversation's
// own id: the session id Talaria created, or the id Hermes answered for a
// completions turn. That pair is what lets the relay stamp the flow the chat
// transport recorded for the conversation onto the call (talaria-gateway
// usage.rs, "Attribution").
//
// Agent-authenticated, and only on the agent's own credential: the org-wide
// key proves fleet membership but not which agent is speaking, and a report
// filed under another agent's name would move that agent's spend. Even on
// its own key the report is trusted no further than the header it explains —
// the key this door takes is readable from the container's terminal, so a
// model can file one itself, and all it can do is move spend between the
// same agent's own flows. Both fields are re-validated here.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::{refuse_legacy, require_agent};
use talaria_api_facades::gateway::usage::{affinity_key, record_affinity_key};
use talaria_body::parse;
use talaria_error::{house_error, internal, object_or_400};
use talaria_state::AppState;

// doc: Report which conversation the affinity key on this agent's LLM calls belongs to.

/// The pair the plugin reports, validated: (affinity key, conversation id).
/// Both are affinity keys in the relay's sense — short printable ASCII — and
/// a pair that names the same thing twice says nothing the relay does not
/// already know. Pure, so the rules are testable without a database.
fn pair_from(body: &Value) -> Result<(String, String), &'static str> {
    let key = affinity_key(body.get("key").and_then(Value::as_str))
        .ok_or("key required: 1..128 printable characters")?;
    let conversation = affinity_key(body.get("conversation").and_then(Value::as_str))
        .ok_or("conversation required: 1..128 printable characters")?;
    if key == conversation {
        return Err("key and conversation are the same — nothing to record");
    }
    Ok((key, conversation))
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    if let Some(refused) = refuse_legacy(&caller, "reporting an affinity key") {
        return Ok(refused);
    }
    let parsed = parse(&body);
    object_or_400(&parsed)?;
    let (key, conversation) = match pair_from(&parsed) {
        Ok(pair) => pair,
        Err(why) => return Ok(house_error(StatusCode::BAD_REQUEST, why)),
    };
    match record_affinity_key(&state.pg, &caller.model, &key, &conversation).await {
        Ok(claimed) => Ok(Json(json!({ "ok": true, "claimed": claimed })).into_response()),
        Err(e) => Ok(internal("[affinity] recording the key failed", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_names_a_key_and_a_different_conversation() {
        let pair = pair_from(&json!({
            "key": "gwk_c937c8957e89c9ea29f55f49",
            "conversation": "api-04a611790172ebea",
        }))
        .expect("a valid pair");
        assert_eq!(pair.0, "gwk_c937c8957e89c9ea29f55f49");
        assert_eq!(pair.1, "api-04a611790172ebea");
        // Trimmed like the header itself.
        assert_eq!(
            pair_from(&json!({ "key": " gwk_x ", "conversation": "c1" }))
                .unwrap()
                .0,
            "gwk_x"
        );
        assert!(pair_from(&json!({ "conversation": "c1" })).is_err());
        assert!(pair_from(&json!({ "key": "gwk_x" })).is_err());
        assert!(pair_from(&json!({ "key": "gwk_x", "conversation": "two words" })).is_err());
        assert!(pair_from(&json!({ "key": "k".repeat(129), "conversation": "c1" })).is_err());
        assert!(pair_from(&json!({ "key": 7, "conversation": "c1" })).is_err());
        assert!(
            pair_from(&json!({ "key": "c1", "conversation": "c1" })).is_err(),
            "a key that is already the conversation's own id needs no record"
        );
    }
}
