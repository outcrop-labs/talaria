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
// re-clamped, secret-scrubbed and run through the chat path's reply guard
// before it becomes a message row (`talaria_cron_output::land`, which the
// cron-output pull lands through as well). Correlation is exact rather than
// approximate (unlike tool-events' newest-live-run): the cron task id
// carries the job id, and the schedule row names the one conversation
// listening to that job for this agent — and the person who scheduled it,
// whom the landed turn is attributed to as its responsible user.
//
// THE PUSH IS NOT THE ONLY ROAD. The plugin's POST is fire-and-forget, so a
// report it lost (an api restart at fire time, a timeout, a full queue) is
// pulled from the agent's own session store within minutes
// (`talaria_cron_output::pull`). The `sessionId` the plugin sends is what the
// two roads dedupe on: it is stamped on the row, and a run the pull already
// landed is not landed again here.
//
// A RUN THAT ANSWERED `[SILENT]` lands nothing: the plugin's hook fires on
// the final text before Hermes's own delivery suppresses it, so the door
// reads the sentinel with Hermes's rule (`talaria_cron_output::is_silence`),
// records the run as delivered silent, and answers `silent: true` with no
// message id.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_auth::{refuse_legacy, require_agent};
use talaria_cron_output::{CronReport, Outcome, Via, land};
use talaria_error::{house_error, internal};
use talaria_state::AppState;

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
    let text = parsed.get("text").and_then(|v| v.as_str()).unwrap_or("");
    // The plugin never sends a blank turn; a hand-rolled caller that does
    // would land an empty assistant row and ring a bell for nothing.
    if text.trim().is_empty() {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "text must not be blank",
        ));
    }
    let marker = turn_marker(
        &job,
        &execution,
        parsed.get("turnId").and_then(|v| v.as_str()).unwrap_or(""),
    );
    // The Hermes session the run is, as the plugin saw it. Absent or blank
    // (an older plugin), the turn lands exactly as it always did, on the
    // marker alone.
    let session_id = parsed
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 200);
    let outcome = match land(
        &state.pg,
        state.redis().await.ok(),
        &listener,
        CronReport {
            agent_model: &caller.model,
            job_id: &job,
            marker,
            session_id,
            via: Via::Push,
            text,
        },
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(e) => return Ok(internal("[cron-output] turn insert failed", e)),
    };
    // One shape for both: `messageId` is the row (null for a quiet run, which
    // wrote none), `silent` says the run was recorded as delivered silent.
    let (message_id, silent) = match outcome {
        Outcome::Landed(landed) => (Some(landed.message_id), false),
        Outcome::Silent { .. } => (None, true),
    };
    Ok(Json(json!({ "ok": true, "messageId": message_id, "silent": silent })).into_response())
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
}
