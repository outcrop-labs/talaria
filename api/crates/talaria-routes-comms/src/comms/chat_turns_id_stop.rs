// /api/chat/turns/{id}/stop.
// POST → stop the turn the page's own send named `id` (`turnId` on
// /api/chat), wherever it has got to. The page sends this the moment Stop is
// pressed on a new thread's first turn, whose thread id rides back only with
// the send's response headers: so the stop no longer waits on the page, and
// a page closed (or a view that starts a new chat) right after Stop no longer
// takes the stop with it. The send itself runs on past a closed page, because
// the app's proxy keeps a write going after its caller leaves.
//
// The stop waits, up to the control window, for the send to come, check its
// caller's access and claim its thread (talaria-chat-persist `turn_stop`),
// then stops that thread's turn exactly as /api/conversations/{id}/stop does.
// A send that has not come within the window gets the stop held: it is
// handed to the send's opening when it comes, and the drain sends it the
// moment the turn names itself.
//
// A turn is only ever its sender's: a stop under a name another person sent,
// or holds, is nothing here, and answers the same 409 as a turn that ended.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::gateway::fleet_chat::CONTROL_WINDOW;
use talaria_chat_persist::{TurnAwaited, await_sent_turn};
use talaria_conversations::accessible_conversation;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;
use tokio::time::Instant;

use super::conversations_id_stop::stop_thread;

// doc: Stop the turn the caller's own send named (`turnId` on /api/chat),
// doc: before or after the api has heard of it: the stop a page sends for a
// doc: new thread, whose id it does not have yet. Waits up to 10 s for the
// doc: send, then stops its thread's turn as /api/conversations/{id}/stop does
// doc: (`hermes`). A send that has not come by then gets the stop `held`,
// doc: for its turn to be stopped when it opens. Only the sender's own turn:
// doc: anything else, or a turn that ended → 409.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // The page draws a uuid; anything else names no send.
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let deadline = Instant::now() + CONTROL_WINDOW;
    let conversation_id = match await_sent_turn(&id, &user.id, None, deadline).await {
        TurnAwaited::Sent { conversation_id } => conversation_id,
        TurnAwaited::Held => {
            tracing::info!(
                "[chat] turn {id}: a person's stop came before its send, which had not come within {}s; the stop is held until it does",
                CONTROL_WINDOW.as_secs()
            );
            return Ok(Json(json!({ "ok": true, "hermes": false, "held": true })).into_response());
        }
        TurnAwaited::Nothing => {
            return Ok(house_error(StatusCode::CONFLICT, "nothing is streaming"));
        }
    };
    // The send checked its caller's access to the thread; the stop asks again,
    // by the same rule as the thread's own stop, and needs its agent anyway.
    let conv = match accessible_conversation(&state.pg, &user.id, &conversation_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(house_error(StatusCode::CONFLICT, "nothing is streaming")),
        Err(e) => return Ok(internal("[chat] access read on turn stop failed", e)),
    };
    Ok(stop_thread(
        &state,
        &user.id,
        &conversation_id,
        &conv.agent_model,
        None,
        deadline,
    )
    .await)
}
