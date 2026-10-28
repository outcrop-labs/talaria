// /api/conversations/{id}/stop.
// POST → interrupt the reply that is streaming in this thread right now. The
// stop button, server-side: Hermes addresses a running turn by its own run id.
// A session turn's run id is on its assistant row (stamped from
// `run.started`). A completions turn has none, but while its request is open
// its completion id (`chatcmpl-…`) is its run id, and the drain reading the
// turn knows it (talaria-chat-persist `open_completion`), so it is stopped
// the same way: Hermes interrupts a started turn, or withdraws one still
// queued behind another of the agent's turns, and ends its stream.
//
// A stop is not a cancel of the turn's record. The row stays, with whatever the
// agent managed to say, and lands `complete` with a `stopped` stamp (the drain
// loop in chat_persist owns that ending, reached by the `run.cancelled` a
// session's container sends after the interrupt, or the `cancelled` finish a
// completion's stream ends with). Nothing here writes the row — asking
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
use talaria_api_facades::gateway::fleet_chat::{stop_completion, stop_run};
use talaria_chat_persist::open_completion;
use talaria_conversations::{accessible_conversation, streaming_run};
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

// doc: Interrupt the reply streaming in this thread, on either arm: a session
// doc: turn by its run id, a completions turn by its completion id. `hermes`
// doc: says whether the agent's own run was interrupted, or whether hanging up
// doc: was the whole stop (a completions turn whose id is not known, which a
// doc: drain in another process would be). Nothing is streaming → 409.

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
    let Some((message_id, run_id)) = streaming else {
        return Ok(house_error(StatusCode::CONFLICT, "nothing is streaming"));
    };
    // NO RUN ID IS THE COMPLETIONS ARM. Its turn names no run, but while the
    // drain holds its request open the completion id the stream named is the
    // run's id in Hermes, so the RUN is stopped, not just this reader: the
    // client's own abort only hangs up its copy of the stream, and the drain
    // reads on server-side, so without this the agent worked (and spent) to
    // the end of a turn nobody wanted. The drain is asked again before every
    // send, so a turn the lost-turn watch moved is stopped where it went.
    let Some(run_id) = run_id else {
        if open_completion(&message_id).is_none() {
            // No drain here is reading it (a restart dropped the request it
            // ran under, or another process holds it): hanging up is all
            // there is, and `hermes: false` says so.
            return Ok(Json(json!({ "ok": true, "hermes": false })).into_response());
        }
        return Ok(
            match stop_completion(&conv.agent_model, || open_completion(&message_id)).await {
                Ok(Some(done)) => {
                    tracing::info!("[conversations] {id}: a person's stop: the turn was {done}");
                    Json(json!({ "ok": true, "hermes": true })).into_response()
                }
                // The stream ended while the stop was on its way: the turn
                // finished on its own, exactly the stale stop a 409 means.
                Ok(None) => house_error(StatusCode::CONFLICT, "nothing is streaming"),
                Err(msg) => house_error(StatusCode::BAD_GATEWAY, &msg),
            },
        );
    };
    match stop_run(&conv.agent_model, &run_id).await {
        Ok(()) => Ok(Json(json!({ "ok": true, "hermes": true })).into_response()),
        // The container refused or never answered. The reason is the closed
        // vocabulary the gateway built from the status and Hermes' error code —
        // never the body's prose, which can carry the agent's own words.
        Err(msg) => Ok(house_error(StatusCode::BAD_GATEWAY, &msg)),
    }
}
