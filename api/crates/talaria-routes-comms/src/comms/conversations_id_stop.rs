// /api/conversations/{id}/stop.
// POST → interrupt the reply that is streaming in this thread right now. The
// stop button, server-side: Hermes addresses a running turn by its own run id,
// and the assistant row is the only place that says which one this is.
//
// A stop is not a cancel of the turn's record. The row stays, with whatever the
// agent managed to say, and lands `complete` with a `stopped` stamp (the drain
// loop in chat_persist owns that ending, reached by the `run.cancelled` the
// container sends after the interrupt). Nothing here writes the row — asking
// twice must cost nothing, and the stream is the one writer.
//
// Gated by the conversations' own access rule — the owner, or a member of the
// plan / work session. Inaccessible and nonexistent answer the same 403, never
// leaking which.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::gateway::fleet_chat::stop_run;
use talaria_conversations::{accessible_conversation, streaming_run};
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

// doc: Interrupt the reply streaming in this thread. `hermes` says whether the
// doc: agent's own run was interrupted, or whether hanging up was the whole
// doc: stop. Nothing is streaming → 409.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // A malformed id would reach Postgres as a failed `::uuid` cast and answer
    // 500 for what is plainly a miss.
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    // The access gate, and the agent in one read: a stop names the container to
    // send it to, and the conversation's own agent is who is talking.
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[conversations] access read on stop failed", e)),
    };
    let streaming = match streaming_run(&state.pg, &id).await {
        Ok(s) => s,
        Err(e) => {
            return Ok(internal(
                "[conversations] streaming-row read on stop failed",
                e,
            ));
        }
    };
    // Nothing in flight: a stop with nothing to stop is the caller's state
    // being stale, not a server fault, so it says so rather than answering ok
    // to a thing it did not do.
    let Some((_message_id, run_id)) = streaming else {
        return Ok(house_error(StatusCode::CONFLICT, "nothing is streaming"));
    };
    // NO RUN ID MEANS THE CLIENT'S OWN ABORT IS THE STOP. A turn driven through
    // the full-history chat arm has no run Hermes could be asked to interrupt —
    // the request is one long response, and dropping the read is how it ends.
    // The persist keeps draining server-side either way, so the row still
    // lands; `hermes: false` is what tells the client its own abort was the
    // whole of it.
    let Some(run_id) = run_id else {
        return Ok(Json(json!({ "ok": true, "hermes": false })).into_response());
    };
    match stop_run(&conv.agent_model, &run_id).await {
        Ok(()) => Ok(Json(json!({ "ok": true, "hermes": true })).into_response()),
        // The container refused or never answered. The reason is the closed
        // vocabulary the gateway built from the status and Hermes' error code —
        // never the body's prose, which can carry the agent's own words.
        Err(msg) => Ok(house_error(StatusCode::BAD_GATEWAY, &msg)),
    }
}
