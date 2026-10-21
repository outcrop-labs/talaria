// POST /api/agents/cron-output. The other inbound half of the talaria-events
// Hermes plugin: a cron run's FINAL TEXT, landed in the conversation that
// scheduled the job, as an ordinary agent turn.
//
// A cron fires the agent's own loop inside the container and answers to
// nobody — the scheduler writes the turn to the agent's own session and the
// platform never sees a word of it. That is the gap this door closes: a
// schedule created from a conversation keeps reporting INTO that
// conversation, so the person who asked for the daily digest reads it where
// they asked for it.
//
// Agent-authenticated, never session-authenticated — the same door as
// /api/agents/tool-events, and the same distrust of the plugin: the text is
// re-clamped and secret-scrubbed HERE, at the boundary, before it becomes a
// message row that models and transcripts will read back, and then run
// through the same reply guard the chat path applies when a streamed reply
// lands (`guard_chat_reply`): a cron turn is nobody's open stream, so this
// door is the only place that pass can happen. Correlation is exact rather
// than approximate (unlike tool-events' newest-live-run): the cron task id
// carries the job id, and the schedule row names the one conversation
// listening to that job for this agent — and the person who scheduled it,
// whom the landed turn is attributed to as its responsible user.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::{refuse_legacy, require_agent};
use talaria_api_facades::gateway::guard::{
    GuardMode, Spread, guard_chat_reply, needs_redaction, redact_findings, redact_secrets,
};
use talaria_body::truncate_utf16;
use talaria_error::{house_error, internal};
use talaria_notify::{NotifyDeps, fan_conversation_event, notify_agent_reply};
use talaria_state::AppState;

const MAX_TEXT: usize = 20_000;

/// The turn's idempotency key, and the only thing that makes this door safe
/// to retry: the plugin is fire-and-forget with a bounded queue, so the same
/// execution can arrive twice (a retried POST, a restarted poster). The
/// marker names the execution, not the clock, so `post_agent_turn` hands the
/// second arrival the first one's message id and writes nothing. `turn_id`
/// is empty when the harness passes none — a cron execution normally has
/// exactly one turn, and the trailing colon keeps the grammar fixed either
/// way.
fn turn_marker(job: &str, execution: &str, turn_id: &str) -> String {
    format!("cron:{job}:{execution}:{turn_id}")
}

/// What the row may hold, regardless of what the plugin clamped to. UTF-16
/// units because that is the length every surface downstream counts in.
fn clamp_text(text: &str) -> &str {
    truncate_utf16(text, MAX_TEXT)
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    // The turn lands under `caller.model`, in a thread that scheduled THAT
    // agent's job: an asserted name (the org-wide key) is not enough to
    // write as it — a shared-key holder naming another agent could post a
    // fabricated "cron reply" into its listening thread.
    if let Some(refused) = refuse_legacy(&caller, "posting a cron reply") {
        return Ok(refused);
    }
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return Ok(house_error(StatusCode::BAD_REQUEST, "body must be JSON")),
    };
    let task_id = parsed.get("taskId").and_then(|v| v.as_str()).unwrap_or("");
    let Some((job, execution)) = talaria_agent_crons::job_id_of_task(task_id) else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "taskId must be cron:<jobId>:<executionId>",
        ));
    };
    // The job→conversation link is scoped to the CALLER's model, so one
    // agent's key can never post into a conversation listening to another
    // agent's schedule.
    let listener =
        match talaria_agent_crons::conversation_for_job(&state.pg, &caller.model, &job).await {
            Ok(Some(l)) => l,
            Ok(None) => {
                return Ok(house_error(
                    StatusCode::NOT_FOUND,
                    "no conversation is listening to this job",
                ));
            }
            Err(e) => return Ok(internal("[cron-output] conversation lookup failed", e)),
        };
    let conversation = listener.conversation_id;
    let text = parsed.get("text").and_then(|v| v.as_str()).unwrap_or("");
    // The plugin never sends a blank turn; a hand-rolled caller that does
    // would land an empty assistant row and ring a bell for nothing.
    if text.trim().is_empty() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "text must not be blank",
        ));
    }
    // Scrub at the boundary — the same shapes the transcript capture kills.
    // A cron turn is a stored, re-readable message, so a key echoed into the
    // agent's closing summary would otherwise outlive the run.
    let text = talaria_api_facades::runs::defs::work_session::scrub_secrets(clamp_text(text));
    // The chat path's landing pass, on the one turn this door lands. No tool
    // names reach here (the cron ran inside the container), so the record
    // honestly says neither half is supplied; a conversation is a contained
    // audience. Strict mode redacts the SAVED copy before it is written —
    // there is no stream to flush, so the row never holds the unredacted
    // text — and annotate/strict pin the scrubbed findings on the row as the
    // caveat the UI renders (transcripts never read that column).
    let (findings, mode) = guard_chat_reply(
        &state.pg,
        &text,
        &[],
        "",
        &format!("cron:{}", caller.model),
        &caller.model,
        Spread::Contained,
    )
    .await;
    let text = if mode == GuardMode::Strict && needs_redaction(&findings) {
        redact_secrets(&text, None).0
    } else {
        text
    };
    let marker = turn_marker(
        &job,
        &execution,
        parsed.get("turnId").and_then(|v| v.as_str()).unwrap_or(""),
    );
    let message_id = match talaria_conversations::post_agent_turn(
        &state.pg,
        &conversation,
        &marker,
        &text,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => return Ok(internal("[cron-output] turn insert failed", e)),
    };
    // THE RESPONSIBLE USER. A cron turn arrives with no person behind it —
    // nobody is chatting — so the turn is attributed to the person who set
    // the schedule up: `author_user_id` is the one slot a turn carries for a
    // human, and the UI labels only user turns from it, so an assistant row
    // gains an accountable person without gaining a byline. A re-delivered
    // execution finds the stamp already there and leaves it.
    if let Some(creator) = listener.created_by.as_deref()
        && let Err(e) = sqlx::query(
            "update messages set author_user_id = $2::uuid \
             where id = $1::uuid and author_user_id is null",
        )
        .bind(&message_id)
        .bind(creator)
        .execute(&state.pg)
        .await
    {
        tracing::error!("[cron-output] responsible-user stamp failed: {e}");
    }
    if !findings.is_empty() && matches!(mode, GuardMode::Annotate | GuardMode::Strict) {
        let _ = talaria_conversations::set_message_guard(
            &state.pg,
            &message_id,
            &redact_findings(&findings),
        )
        .await;
    }
    // Exactly what the chat persist path does once a reply lands complete:
    // every reader's thread pill moves, and whoever was away gets the one
    // agent-reply row. Both are id-shaped and both are detached — a cron
    // turn is nobody's open stream, so the fan-out is the ONLY way this
    // conversation learns it moved. A re-delivered execution re-fires them
    // harmlessly: the event carries no content, and the inbox row dedupes
    // while one sits unread per thread.
    fan_conversation_event(
        NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok()),
        conversation.clone(),
    );
    {
        let notify = NotifyDeps::publishing(state.pg.clone(), state.redis().await.ok());
        let conversation = conversation.clone();
        let message_id = message_id.clone();
        tokio::spawn(async move {
            notify_agent_reply(&notify, &conversation, &message_id).await;
        });
    }
    Ok(Json(json!({ "ok": true, "messageId": message_id })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The marker's spelling IS the idempotency contract — a schedule's
    // second delivery of the same execution has to collide with the first,
    // and a different execution must not. Pinned here because the only
    // other place it is written down is the conversation row it already
    // wrote.
    #[test]
    fn marker_names_the_execution() {
        assert_eq!(turn_marker("abc123", "ff01", "t7"), "cron:abc123:ff01:t7");
        // No turn id from the harness: the trailing colon stays, so the
        // grammar has four fields whether or not the fourth is filled.
        assert_eq!(turn_marker("abc123", "ff01", ""), "cron:abc123:ff01:");
        // Different execution of the same job — a new turn, not a repeat.
        assert_ne!(
            turn_marker("abc123", "ff01", ""),
            turn_marker("abc123", "ff02", "")
        );
    }

    #[test]
    fn text_is_clamped_at_the_boundary() {
        let short = "the digest";
        assert_eq!(clamp_text(short), short);
        let long = "x".repeat(MAX_TEXT + 500);
        assert_eq!(clamp_text(&long).len(), MAX_TEXT);
        // UTF-16 units, not bytes or chars: an emoji is a surrogate pair, so
        // half the cap's worth of them fills it exactly and the cut never
        // lands inside a character.
        let emoji = "🛰".repeat(MAX_TEXT);
        let cut = clamp_text(&emoji);
        assert_eq!(cut.chars().count(), MAX_TEXT / 2);
        assert!(emoji.starts_with(cut));
    }
}
