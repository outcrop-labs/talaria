// /api/channels/{id}/messages/{msgId}/reactions.
// POST { emoji } → toggle your reaction on a message. Agents react too, under
// their own identity — one of our twists on the Slack shape.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::{AgentSubject, agent_caller};
use talaria_body::string_member;
use talaria_channels::{
    ChannelIntent, agent_may_access_channel, channel_role, get_channel_message, toggle_reaction,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_ratelimit::brake;
use talaria_session::require_user;
use talaria_state::AppState;

/// Reactions an agent may toggle per minute.
const AGENT_REACTIONS_PER_MINUTE: i64 = 60;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, msg_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    // The body parses FIRST — a malformed emoji outranks every gate.
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    let emoji = match string_member(obj, "emoji", 1, 16) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    if let Some(gate) = talaria_params::uuid_gate_404(&msg_id) {
        return Ok(gate);
    }
    // WHO ASKS, BEFORE WHAT EXISTS. The message lookup used to run ahead of
    // authentication, which made this route an unauthenticated oracle for
    // (channel, message) existence. Now the caller is known — and may reach
    // the channel — before the row is read; an outsider sees 401/403 and
    // never learns whether the id was real.
    let caller = agent_caller(&state.pg, &headers).await?;
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Some(caller) = caller {
        let name = caller.model.clone();
        // The CALLER, not `name`: elevation buys org-wide channel reach.
        // A reaction is a WRITE on the channel: it asks as a post.
        let may = match agent_may_access_channel(
            &state.pg,
            &id,
            &AgentSubject::Caller(caller),
            ChannelIntent::Post,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                return Ok(internal(
                    "[channels] agent access read on reactions failed",
                    e,
                ));
            }
        };
        if !may {
            return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
        }
        if let Ok(mut redis) = state.redis().await
            && let Some(gate) = brake(
                &mut redis,
                &format!("agent:reaction:{name}"),
                AGENT_REACTIONS_PER_MINUTE,
                60,
                "too many reactions from this agent — wait before reacting again",
            )
            .await
        {
            return Ok(gate);
        }
        if !message_exists(&state, &id, &msg_id).await? {
            return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
        }
        return Ok(
            match toggle_reaction(&notify, &id, &msg_id, &emoji, &name, "agent").await {
                Ok(()) => Json(json!({ "ok": true })).into_response(),
                Err(e) => internal("[channels] agent reaction failed", e),
            },
        );
    }
    let user = require_user(&state, &headers).await?;
    match channel_role(&state.pg, &user.id, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[channels] role read on reactions failed", e)),
    }
    if !message_exists(&state, &id, &msg_id).await? {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }
    let actor = talaria_conversations::reaction_actor(user.email.as_deref(), user.name.as_deref());
    Ok(
        match toggle_reaction(&notify, &id, &msg_id, &emoji, &actor, "user").await {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(e) => internal("[channels] reaction failed", e),
        },
    )
}

/// Is `msg_id` a message of channel `id`? Asked only once the caller is
/// known to reach the channel. The Err is the route's own 500.
async fn message_exists(state: &AppState, id: &str, msg_id: &str) -> Result<bool, Response> {
    match get_channel_message(&state.pg, id, msg_id).await {
        Ok(m) => Ok(m.is_some()),
        Err(e) => Err(internal("[channels] message read on reactions failed", e)),
    }
}
