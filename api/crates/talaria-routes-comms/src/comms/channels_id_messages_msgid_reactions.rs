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
    agent_may_access_channel, channel_role, get_channel_message, toggle_reaction,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::NotifyDeps;
use talaria_session::require_user;
use talaria_state::AppState;

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
    let msg = match get_channel_message(&state.pg, &id, &msg_id).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[channels] message read on reactions failed", e)),
    };
    if msg.is_none() {
        return Ok(house_error(StatusCode::NOT_FOUND, "not found"));
    }

    let caller = agent_caller(&state.pg, &headers).await?;
    let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
    if let Some(caller) = caller {
        let name = caller.model.clone();
        // The CALLER, not `name`: elevation buys org-wide channel reach.
        let may =
            match agent_may_access_channel(&state.pg, &id, &AgentSubject::Caller(caller)).await {
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
    let actor = user
        .email
        .clone()
        .or_else(|| user.name.clone())
        .unwrap_or_else(|| "user".into());
    Ok(
        match toggle_reaction(&notify, &id, &msg_id, &emoji, &actor, "user").await {
            Ok(()) => Json(json!({ "ok": true })).into_response(),
            Err(e) => internal("[channels] reaction failed", e),
        },
    )
}
