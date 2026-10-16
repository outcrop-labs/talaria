// /api/conversations/{id}/steer.
// POST { content } → say something to the agent WHILE it is replying, and have
// it account for it in the turn it is already in. Hermes hands a steer to a
// running agent mid-turn; without it, a correction typed over a streaming reply
// can only wait for the reply to finish being wrong.
//
// THE WORDS LAND IN THE TRANSCRIPT EITHER WAY. The user row is written before
// the steer is attempted, stamped `{ steer: true }`, because what a person said
// mid-run is part of what was said in this thread — a steer the container
// declined must not vanish, and a turn that answers it must be able to read it
// back. That is also the fallback: a row already in history is a queued
// message, and the completing turn's chain picks it up exactly as it picks up
// anything sent while a reply streams.
//
// Gated by the conversations' own access rule — the owner, or a member of the
// plan / work session. Inaccessible and nonexistent answer the same 403, never
// leaking which.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_api_facades::gateway::fleet_chat::{SteerOutcome, steer_run};
use talaria_body::string_member;
use talaria_conversations::{
    accessible_conversation, insert_user_message, mark_steer_accepted, next_seq, streaming_run,
    touch_conversation,
};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::require_user;
use talaria_state::AppState;

// doc: Say something to the agent mid-reply. `steered` means the running turn
// doc: took the words; `queued` (202) means it did not and the row waits in
// doc: history for the next turn. Nothing is streaming → 409.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // A malformed id would reach Postgres as a failed `::uuid` cast and answer
    // 500 for what is plainly a miss.
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    // The access gate, and the agent in one read: the steer names the container
    // to push the words into, and the conversation's own agent is who is
    // talking.
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => return Ok(internal("[conversations] access read on steer failed", e)),
    };
    let parsed = talaria_body::parse(&body);
    let obj = object_or_400(&parsed)?;
    // content: what to say to the agent mid-turn — a sentence of redirection,
    // not a document, which is what the small ceiling says.
    let content = match string_member(obj, "content", 1, 20_000) {
        Ok(c) => c,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // ASKED BEFORE THE ROW IS WRITTEN, on purpose. With nothing in flight there
    // is no turn to steer and the sender meant to send a message — answering
    // that plainly is better than landing a row the sender never meant to
    // queue and then refusing anyway.
    let streaming = match streaming_run(&state.pg, &id).await {
        Ok(s) => s,
        Err(e) => {
            return Ok(internal(
                "[conversations] streaming-row read on steer failed",
                e,
            ));
        }
    };
    let Some((_message_id, run_id)) = streaming else {
        return Ok(house_error(
            StatusCode::CONFLICT,
            "nothing is streaming to steer — send it as a message",
        ));
    };
    let seq = match next_seq(&state.pg, &id).await {
        Ok(s) => s,
        Err(e) => return Ok(internal("[conversations] seq read on steer failed", e)),
    };
    // The authenticated row is the attribution — the same rule /api/chat
    // follows: who the session says is typing, never who the client claims. A
    // steer carries no attachments, and the stamp is what marks the row as
    // having been said into a running turn rather than at the top of one.
    let message_id = match insert_user_message(
        &state.pg,
        &id,
        seq,
        &content,
        &Value::Array(Vec::new()),
        Some(&user.id),
        &json!({ "steer": true }),
    )
    .await
    {
        Ok(m) => m,
        Err(e) => return Ok(internal("[conversations] steer turn persist failed", e)),
    };
    let _ = touch_conversation(&state.pg, &id, None).await;

    // THE QUEUED ANSWER, and the reason nothing is spawned behind it: the read
    // above said a reply is in flight, so the turn that completes picks this
    // row up through its own chain — the same thing /api/chat does when a
    // message lands while a reply streams. Spawning a continuation here would
    // be a second turn racing the one already running.
    let queued = || {
        (
            StatusCode::ACCEPTED,
            Json(json!({ "queued": true, "messageId": message_id })),
        )
            .into_response()
    };
    // No run id: the turn in flight went through the full-history chat arm,
    // which Hermes addresses by nothing a steer could name. The row is the
    // whole of what this route can do, and that is the queued answer.
    let Some(run_id) = run_id else {
        return Ok(queued());
    };
    match steer_run(&conv.agent_model, &run_id, &content).await {
        SteerOutcome::Accepted => {
            // Answered by the turn it went into: the stamp is what keeps the
            // completing turn's chain from answering it a second time.
            let _ = mark_steer_accepted(&state.pg, &message_id).await;
            Ok(Json(json!({ "steered": true, "messageId": message_id })).into_response())
        }
        // Past taking it (the run finished between the read and the push, or
        // the agent declined), or the attempt never got there. Both leave the
        // row in history, which is the outcome the queued answer describes —
        // a failure is logged rather than returned, because the sender's words
        // did land and a 502 would say they were lost.
        SteerOutcome::NotAccepting => Ok(queued()),
        SteerOutcome::Failed(e) => {
            tracing::warn!("[conversations] {id}: steering run {run_id} failed: {e}");
            Ok(queued())
        }
    }
}
