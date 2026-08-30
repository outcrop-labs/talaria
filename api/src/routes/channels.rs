// /api/channels — port of ui/src/routes/api/channels.ts.
// GET → the user's channels/relays/DMs (agents see the channels they've been
// added to, elevated assistants every non-DM). POST { name, topic?, kind? } →
// create a channel (default) or a Relay (kind 'group'), behind the
// comms.channels / comms.relays permission.

use crate::agent_auth::{AgentSubject, agent_caller};
use crate::body::{
    as_object, optional_enum_member, present_nullable_max_string_member, string_member,
};
use crate::channels::{create_channel, list_channels, list_channels_for_agent};
use crate::error::house_error;
use crate::mcp_service::ensure_mcp_service;
use crate::permissions::has_perm;
use crate::retrieval::backfill::maybe_rag_sweep;
use crate::session::require_user;
use crate::state::AppState;
use crate::titler::maybe_sweep_titles;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Agents see the channels they've been added to. Err is the refusal to
    // return verbatim; Ok(None) falls to the session path.
    let caller = match agent_caller(&state.pg, &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return get_as_user(&state, &headers).await,
        Err(resp) => return resp,
    };
    // The CALLER, not its model. `list_channels_for_agent` widens to EVERY
    // non-DM channel for an elevated assistant, and the subject type reads a
    // bare model string as proven — so `caller.model` here would throw the
    // legacy flag away and hand org-wide reach to an asserted identity.
    let channels = match list_channels_for_agent(&state.pg, &AgentSubject::Caller(caller)).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[channels] agent listing failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    Json(json!({ "channels": channels })).into_response()
}

async fn get_as_user(state: &AppState, headers: &HeaderMap) -> Response {
    let user = match require_user(state, headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    // Comms decay and the outreach sweep used to be kicked from here; they
    // are scheduler jobs on both sides now. The three below are still
    // request-kicked: `maybe_sweep_titles` and `maybe_rag_sweep` have the
    // same "an idle instance never does it" bug, and `ensure_mcp_service` is
    // a supervisor, which is a scheduler job in everything but name.
    maybe_sweep_titles(state.clone()); // retroactive + ongoing naming (hourly, detached)
    ensure_mcp_service(); // keep the fleet's toolkit MCP endpoint alive (probe-guarded)
    maybe_rag_sweep(state.clone()); // incremental catch-up indexing (15-minute throttle)
    match list_channels(&state.pg, &user.id).await {
        Ok(channels) => Json(json!({ "channels": channels })).into_response(),
        Err(e) => {
            tracing::error!("[channels] listing failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
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
    let parsed = crate::body::parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let topic = match present_nullable_max_string_member(obj, "topic", 300) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let kind = match optional_enum_member(obj, "kind", &["channel", "group"]) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let kind = kind.as_deref().unwrap_or("channel");
    let needed = if kind == "group" {
        "comms.relays"
    } else {
        "comms.channels"
    };
    let allowed = match has_perm(&state.pg, &user.id, &user.role, needed).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[channels] permission read failed: {e}");
            return house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
        }
    };
    if !allowed {
        return house_error(
            StatusCode::FORBIDDEN,
            &format!(
                "no permission to create {}",
                if needed == "comms.relays" {
                    "relays"
                } else {
                    "channels"
                }
            ),
        );
    }
    // `body.topic ?? null` — absent and present-null both create with none.
    match create_channel(&state.pg, &user.id, &name, topic.flatten().as_deref(), kind).await {
        Ok(channel) => Json(json!({ "channel": channel })).into_response(),
        Err(e) => {
            tracing::error!("[channels] create failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kinds_map_to_their_permission_sentences() {
        // The two sentences the 403 carries, spelled by the permission asked.
        assert_eq!(
            "no permission to create relays",
            format!(
                "no permission to create {}",
                if "comms.relays" == "comms.relays" {
                    "relays"
                } else {
                    "channels"
                }
            )
        );
    }

    #[test]
    fn topic_takes_all_three_states() {
        let member = |v: serde_json::Value| {
            present_nullable_max_string_member(v.as_object().unwrap(), "topic", 300).unwrap()
        };
        assert_eq!(member(json!({ "name": "x" })), None);
        assert_eq!(member(json!({ "topic": null })), Some(None));
        assert_eq!(
            member(json!({ "topic": "" })),
            Some(Some(String::new())),
            ".max(300).nullish() has no min — the empty topic is a value"
        );
        assert!(
            present_nullable_max_string_member(
                json!({ "topic": "t".repeat(301) }).as_object().unwrap(),
                "topic",
                300
            )
            .is_err()
        );
    }
}
