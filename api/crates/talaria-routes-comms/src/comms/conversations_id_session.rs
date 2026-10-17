// GET /api/conversations/{id}/session — what the agent is still holding of
// this thread.
//
// A PINNED agent keeps the transcript itself: Talaria's conversation id IS the
// Hermes session id, and Hermes compacts the old turns away on its own clock.
// The surface above this needs to be able to say so — "the agent summarized
// everything before Tuesday" — and nothing in Talaria's own tables knows it,
// because the compaction happens inside the container.
//
// WHAT CROSSES THE WIRE IS A COUNT AND A BOUNDARY. Hermes projects a
// compaction carrier as a row with `display_kind: "hidden"` and empty content
// — the summary text never leaves the container — so this route can only ever
// answer how many rows were folded away and which surviving row is now the
// oldest. That is the honest shape, and it is also the safe one.
//
// EVERY "NO" IS THE SAME 200. `applies: false` with zeros and nulls covers an
// unpinned agent, a conversation whose history Hermes never saw (the
// full-history path owns that transcript), and a container that is not
// answering. A drawer asks this on every thread it opens; a 404 or a 502 there
// would put an error in somebody's console for an ordinary chat. The one
// refusal is the access gate — inaccessible and nonexistent answer the same
// 403, never leaking which, the house rule on this plane.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_api_facades::gateway::fleet_chat::session_overview;
use talaria_chat_persist::pinned_turn;
use talaria_conversations::accessible_conversation;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

/// The answer for every thread this does not describe. Zeros and nulls, 200:
/// one shape the caller renders or ignores, never a branch on a status.
fn not_applicable() -> Response {
    Json(json!({
        "applies": false,
        "compacted": 0,
        "since": Value::Null,
        "sessionId": Value::Null,
        "messageCount": Value::Null,
        "partial": false,
        // Null, not false: nobody asked the container on this path. False
        // is reserved for "asked and got no answer", which this door cannot
        // tell apart from the other reasons yet.
        "reachable": Value::Null,
    }))
    .into_response()
}

// doc: What the agent itself still holds of this thread: how many messages it
// doc: has compacted away and how far back the surviving transcript reaches.
// doc: `applies` is false — with zeros and nulls — for an agent that is not
// doc: pinned, a thread Hermes never held, or a container that is not
// doc: answering; the compacted summaries themselves never cross the wire.

pub async fn get(
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
    // The access gate and the agent in one read — the same predicate every
    // other conversation-scoped route uses, and the model names the container
    // to ask.
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            return Ok(internal(
                "[conversations] access read on session overview failed",
                e,
            ));
        }
    };
    // THE CHAT PATH'S OWN QUESTION, asked the chat path's way: pinned on the
    // def, and a history Hermes could answer from. A conversation that ran on
    // the full-history path before the flip has a transcript Hermes never saw,
    // and its session — if one exists at all — describes a different thread
    // than the one on screen.
    if !pinned_turn(&state.pg, &conv.agent_model, &id, None).await {
        return Ok(not_applicable());
    }
    // Hermes is the only one who knows; an unreachable container is a "no",
    // not an error. `reachable` is true only here, where the container
    // answered; every other "no" leaves it null, because nothing was asked.
    let Some(overview) = session_overview(&conv.agent_model, &id).await else {
        return Ok(not_applicable());
    };
    Ok(Json(json!({
        "applies": true,
        "compacted": overview.compacted,
        "since": overview.since,
        "sessionId": overview.session_id,
        "messageCount": overview.message_count,
        "partial": overview.partial,
        "reachable": true,
    }))
    .into_response())
}
