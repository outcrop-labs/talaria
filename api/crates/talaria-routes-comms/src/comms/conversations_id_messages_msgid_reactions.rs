// /api/conversations/{id}/messages/{msgId}/reactions.
// POST { emoji } → toggle your reaction on a message in an agent DM (or any
// conversation you can read): add it if you haven't, remove it if you have.
// Answers { ok: true, reacted } — `reacted` is whether your reaction is now on.
//
// Access is the conversation READ rule (owner, or a plan/work member, or a
// research reader), and a refusal is the same 404 the read answers, so the
// route never confirms a conversation exists to someone who cannot see it. A
// message id from a different conversation is 404 too. Only people react
// here; the actor is the same identity channel reactions record (email, else
// name), so one chip component decides "mine" for both transcripts.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::string_member;
use talaria_conversations::{
    conversation_accessible, conversation_has_message, reaction_actor, toggle_message_reaction,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_notify::{NotifyDeps, fan_conversation_event};
use talaria_session::require_user;
use talaria_state::AppState;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, msg_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
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
    match conversation_accessible(&state.pg, &user.id, &id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => {
            return Ok(internal(
                "[conversations] access read on reactions failed",
                e,
            ));
        }
    }
    match conversation_has_message(&state.pg, &id, &msg_id).await {
        Ok(true) => {}
        Ok(false) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => {
            return Ok(internal(
                "[conversations] message read on reactions failed",
                e,
            ));
        }
    }
    let actor = reaction_actor(user.email.as_deref(), user.name.as_deref());
    let reacted = match toggle_message_reaction(&state.pg, &msg_id, &emoji, &actor, "user").await {
        Ok(on) => on,
        Err(e) => return Ok(internal("[conversations] reaction failed", e)),
    };
    // Everyone with the thread open refreshes it — a plan's collaborators see
    // the chip move without a reload.
    fan_conversation_event(
        NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
        id,
    );
    Ok(Json(json!({ "ok": true, "reacted": reacted })).into_response())
}
