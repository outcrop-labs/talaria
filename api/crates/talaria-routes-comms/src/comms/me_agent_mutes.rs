// /api/me/agent-mutes. The caller's timed mutes of an agent's direct
// messages — its replies in their chat threads with it and its own check-ins;
// its replies in a plan, research, work session or ticket still ring
// (talaria_notify::agent_mutes). GET → { mutes: [{ agentModel, mutedUntil }] },
// only the mutes that still hold. PUT { agentModel, mutedUntil: ISO|null } →
// { ok, agentModel, mutedUntil }: an ISO time in the future mutes until then
// ("until I turn it back on" is the far end, 9999-12-31, as for a channel),
// null unmutes; a write also prunes the caller's expired mutes. Muted rows
// still land in-app, quietly: no toast, push or mail. The caller's own setting, so it names any agent model without an
// access check — a mute of an agent that never writes to them does nothing.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_agent_auth::now_ms as wall_ms;
use talaria_body::{string_member, zod_type_name};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{FOREVER_MS, list_agent_dm_mutes, parse_instant, set_agent_dm_mute};
use talaria_session::require_user;
use talaria_state::AppState;

// The pin grammar's agent bound — one spelling of how long a model may be.
use talaria_conversation_layout::pins::MAX_AGENT_MODEL as MAX_MODEL;

/// The validated body: the agent, and the mute's end (None unmutes).
#[derive(Debug, PartialEq, Eq)]
struct MuteBody {
    agent_model: String,
    muted_until: Option<i64>,
}

fn validate(obj: &serde_json::Map<String, Value>, now_ms: i64) -> Result<MuteBody, String> {
    let agent_model = string_member(obj, "agentModel", 1, MAX_MODEL)?;
    if agent_model.chars().any(char::is_control) {
        return Err("agentModel must not hold control characters".into());
    }
    let muted_until = match obj.get("mutedUntil") {
        None => return Err("mutedUntil is required: an ISO time, or null to unmute".into()),
        Some(Value::Null) => None,
        Some(Value::String(s)) => match parse_instant(s) {
            Some(ms) if ms > now_ms && ms <= FOREVER_MS => Some(ms),
            Some(_) => return Err("mutedUntil must be in the future".into()),
            None => return Err("mutedUntil must be an ISO time, or null".into()),
        },
        Some(v) => {
            return Err(format!(
                "Invalid input: expected string, received {}",
                zod_type_name(v)
            ));
        }
    };
    Ok(MuteBody {
        agent_model,
        muted_until,
    })
}

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    Ok(match list_agent_dm_mutes(&state.pg, &user.id).await {
        Ok(rows) => {
            let mutes: Vec<Value> = rows
                .into_iter()
                .map(
                    |(model, ms)| json!({ "agentModel": model, "mutedUntil": epoch_ms_to_iso(ms) }),
                )
                .collect();
            Json(json!({ "mutes": mutes })).into_response()
        }
        Err(e) => internal("[me/agent-mutes] read failed", e),
    })
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let b = match validate(obj, wall_ms()) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
        match set_agent_dm_mute(&state.pg, &user.id, &b.agent_model, b.muted_until).await {
            Ok(held) => Json(json!({
                "ok": true,
                "agentModel": b.agent_model,
                "mutedUntil": held.map(epoch_ms_to_iso),
            }))
            .into_response(),
            Err(e) => internal("[me/agent-mutes] write failed", e),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(j: Value, now: i64) -> Result<MuteBody, String> {
        validate(j.as_object().unwrap(), now)
    }

    #[test]
    fn the_body_is_pinned() {
        let now = 1_800_000_000_000;
        assert_eq!(
            v(json!({ "agentModel": "dev", "mutedUntil": null }), now).unwrap(),
            MuteBody {
                agent_model: "dev".into(),
                muted_until: None
            }
        );
        assert_eq!(
            v(
                json!({ "agentModel": "dev", "mutedUntil": "9999-12-31T23:59:59.999Z" }),
                now
            )
            .unwrap()
            .muted_until,
            Some(FOREVER_MS)
        );
        assert_eq!(
            v(json!({ "agentModel": "dev" }), now).unwrap_err(),
            "mutedUntil is required: an ISO time, or null to unmute"
        );
        assert_eq!(
            v(
                json!({ "agentModel": "dev", "mutedUntil": "2020-01-01T00:00:00.000Z" }),
                now
            )
            .unwrap_err(),
            "mutedUntil must be in the future"
        );
        assert_eq!(
            v(json!({ "agentModel": "dev", "mutedUntil": "soon" }), now).unwrap_err(),
            "mutedUntil must be an ISO time, or null"
        );
        assert_eq!(
            v(json!({ "agentModel": "dev", "mutedUntil": 5 }), now).unwrap_err(),
            "Invalid input: expected string, received number"
        );
        assert!(v(json!({ "agentModel": "", "mutedUntil": null }), now).is_err());
        assert!(v(json!({ "agentModel": "a\u{7}", "mutedUntil": null }), now).is_err());
        assert!(v(json!({ "mutedUntil": null }), now).is_err());
    }
}
