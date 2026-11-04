// The step-snapshot frame on the tool-events door: `kind: "snapshot"`.
//
// The agent's plugin posts one of these per model step of a work session:
// the batch of tool calls the model just made, and the git snapshot of the
// job's checkout taken once the batch had finished. It lands as a `step` row
// in `run_checkpoints`, keyed by the batch's last tool call id, with the
// turn Talaria is on as a provisional place; the record stage of the
// session stamps the transcript's own order later (`place_steps`).
//
// The door trusts the plugin for nothing it can check. The run is the live
// work session the session id names, as for a tool frame; the job has to be
// this agent's, or the row records that it is not and is unrestorable; a
// frame the ledger already holds answers landed without a second row. A
// frame that names no live run, or fails to write, is answered `landed:
// false` and dropped: observability yields to the agent, and a snapshot
// failure never fails a run.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::AgentCaller;
use talaria_body::truncate_utf16;
use talaria_error::house_error;
use talaria_runs_store::{StepSnapshot, record_step};
use talaria_state::AppState;

const MAX_TITLE: usize = 200;
const MAX_REASON: usize = 400;
const MAX_ID: usize = 200;

/// One finished tool call of the batch, as the plugin saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    id: String,
    name: String,
    failed: bool,
}

/// The frame, read from the body and bounded.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frame {
    session_id: Option<String>,
    turn_id: Option<String>,
    calls: Vec<Call>,
    job_id: Option<String>,
    commit: Option<String>,
    tree: Option<String>,
    reason: Option<String>,
    at_ms: i64,
    snapshot_ms: Option<i64>,
}

fn bounded(v: Option<&Value>, limit: usize) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_utf16(s, limit).to_string())
}

fn object_id(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|s| s.to_ascii_lowercase())
}

fn read_frame(body: &Value, now_ms: i64) -> Option<Frame> {
    let calls: Vec<Call> = body
        .get("calls")?
        .as_array()?
        .iter()
        .filter_map(|c| {
            Some(Call {
                id: bounded(c.get("id"), MAX_ID)?,
                name: bounded(c.get("name"), 120)?,
                failed: c.get("status").and_then(Value::as_str) == Some("error"),
            })
        })
        .collect();
    if calls.is_empty() {
        return None;
    }
    Some(Frame {
        session_id: bounded(body.get("sessionId"), MAX_ID)
            .filter(|s| uuid::Uuid::parse_str(s).is_ok()),
        turn_id: bounded(body.get("turnId"), MAX_ID),
        calls,
        job_id: bounded(body.get("jobId"), MAX_ID).filter(|s| uuid::Uuid::parse_str(s).is_ok()),
        commit: object_id(body.get("commit")),
        tree: object_id(body.get("tree")),
        reason: bounded(body.get("reason"), MAX_REASON),
        at_ms: body.get("at").and_then(Value::as_i64).unwrap_or(now_ms),
        snapshot_ms: body.get("ms").and_then(Value::as_i64),
    })
}

/// The title of a batch of more than one call: the distinct tool names in
/// order, each with its count. One call needs no title; its tool name is the
/// row's.
fn title_for(calls: &[Call]) -> Option<String> {
    if calls.len() < 2 {
        return None;
    }
    let mut names: Vec<(&str, usize)> = Vec::new();
    for call in calls {
        match names.iter_mut().find(|(n, _)| *n == call.name) {
            Some((_, count)) => *count += 1,
            None => names.push((call.name.as_str(), 1)),
        }
    }
    let parts: Vec<String> = names
        .iter()
        .map(|(n, c)| {
            if *c > 1 {
                format!("{n} ×{c}")
            } else {
                n.to_string()
            }
        })
        .collect();
    Some(
        truncate_utf16(
            &format!("{} calls: {}", calls.len(), parts.join(", ")),
            MAX_TITLE,
        )
        .to_string(),
    )
}

/// The job id as the ledger may hold it: this agent's own, or None with the
/// reason the row carries instead.
async fn own_job(
    state: &AppState,
    caller: &AgentCaller,
    job_id: Option<&str>,
) -> Result<Option<String>, sqlx::Error> {
    let Some(job_id) = job_id else {
        return Ok(None);
    };
    sqlx::query_scalar(
        "select j.id::text from workbench_jobs j \
         join agent_defs d on d.id = j.agent_id \
         where j.id = $1::uuid and d.model = $2",
    )
    .bind(job_id)
    .bind(&caller.model)
    .fetch_optional(&state.pg)
    .await
}

fn step_of(frame: &Frame, job_id: Option<String>) -> StepSnapshot {
    let last = frame.calls.last().expect("a frame holds at least one call");
    let reason = match (&frame.reason, &frame.job_id, &job_id) {
        (Some(reason), _, _) => Some(reason.clone()),
        (None, Some(named), None) => Some(format!("job {named} is not this agent's")),
        _ => None,
    };
    StepSnapshot {
        agent_turn_id: frame.turn_id.clone(),
        tool_call_id: last.id.clone(),
        tool: last.name.clone(),
        title: title_for(&frame.calls),
        status: if frame.calls.iter().any(|c| c.failed) {
            "failed".into()
        } else {
            "completed".into()
        },
        job_id,
        workspace_commit: frame.commit.clone(),
        workspace_tree: frame.tree.clone(),
        unrestorable_reason: reason,
        at_ms: frame.at_ms,
    }
}

pub async fn land(
    state: &AppState,
    caller: &AgentCaller,
    body: Value,
) -> Result<Response, Response> {
    let Some(frame) = read_frame(&body, talaria_agent_auth::now_ms()) else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "a snapshot names its calls",
        ));
    };
    let run_id = match &frame.session_id {
        Some(session_id) => {
            talaria_api_facades::runs::defs::work_session::live_work_session_for(
                &state.pg,
                &caller.model,
                session_id,
            )
            .await
        }
        None => Ok(None),
    };
    let run_id = match run_id {
        Ok(Some(run_id)) => run_id,
        Ok(None) => return Ok(Json(json!({ "ok": true, "landed": false })).into_response()),
        Err(e) => {
            tracing::warn!("[tool-events] snapshot run lookup failed: {e}");
            return Ok(Json(json!({ "ok": true, "landed": false })).into_response());
        }
    };
    let job_id = match own_job(state, caller, frame.job_id.as_deref()).await {
        Ok(job_id) => job_id,
        Err(e) => {
            tracing::warn!("[tool-events] snapshot job lookup failed: {e}");
            None
        }
    };
    let step = step_of(&frame, job_id);
    match record_step(&state.pg, &run_id, &step).await {
        Ok(Some(seq)) => {
            tracing::info!(
                "[tool-events] {} run {run_id}: step {seq} at {} ({}) restorable={} snapshot {} ms",
                caller.model,
                step.tool_call_id,
                step.tool,
                step.job_id.is_some()
                    && step.workspace_commit.is_some()
                    && step.unrestorable_reason.is_none(),
                frame.snapshot_ms.unwrap_or(0),
            );
            Ok(Json(json!({ "ok": true, "landed": true, "seq": seq })).into_response())
        }
        Ok(None) => {
            Ok(Json(json!({ "ok": true, "landed": true, "duplicate": true })).into_response())
        }
        Err(e) => {
            tracing::warn!("[tool-events] snapshot for run {run_id} did not land: {e}");
            Ok(Json(json!({ "ok": true, "landed": false })).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: Value) -> Frame {
        read_frame(&body, 1_700_000_000_000).expect("a frame with calls")
    }

    #[test]
    fn a_step_row_is_keyed_by_the_batch_s_last_call_and_titled_by_the_batch() {
        let f = frame(json!({
            "sessionId": "7b1e4c1a-9f8d-8a1b-9c2d-3e4f5a6b7c8d",
            "turnId": "turn-9",
            "calls": [
                {"id": "call_1", "name": "terminal", "status": "completed"},
                {"id": "call_2", "name": "terminal", "status": "error"},
                {"id": "call_3", "name": "write_file", "status": "completed"},
            ],
            "jobId": "2f3b0a6e-1c4d-4e5f-8a9b-0c1d2e3f4a5b",
            "commit": "A1B2C3D4E5F60718293A4B5C6D7E8F9012345678",
            "tree": "0123456789abcdef0123456789abcdef01234567",
            "at": 1_700_000_000_123i64,
            "ms": 24,
        }));
        let step = step_of(&f, Some("2f3b0a6e-1c4d-4e5f-8a9b-0c1d2e3f4a5b".into()));
        assert_eq!(step.tool_call_id, "call_3");
        assert_eq!(step.tool, "write_file");
        assert_eq!(
            step.title.as_deref(),
            Some("3 calls: terminal ×2, write_file")
        );
        assert_eq!(step.status, "failed", "one failed call fails the step");
        assert_eq!(
            step.workspace_commit.as_deref(),
            Some("a1b2c3d4e5f60718293a4b5c6d7e8f9012345678")
        );
        assert_eq!(step.unrestorable_reason, None);
        assert_eq!(step.at_ms, 1_700_000_000_123);
        assert_eq!(step.agent_turn_id.as_deref(), Some("turn-9"));
    }

    #[test]
    fn one_call_has_no_title_and_a_job_that_is_not_the_agent_s_makes_the_step_unrestorable() {
        let f = frame(json!({
            "calls": [{"id": "call_1", "name": "terminal"}],
            "jobId": "2f3b0a6e-1c4d-4e5f-8a9b-0c1d2e3f4a5b",
            "commit": "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
        }));
        let step = step_of(&f, None);
        assert_eq!(step.title, None);
        assert_eq!(step.status, "completed");
        assert_eq!(step.job_id, None);
        assert_eq!(
            step.unrestorable_reason.as_deref(),
            Some("job 2f3b0a6e-1c4d-4e5f-8a9b-0c1d2e3f4a5b is not this agent's")
        );
        assert_eq!(
            step.at_ms, 1_700_000_000_000,
            "no clock from the plugin takes the door's"
        );
    }

    #[test]
    fn the_plugin_s_own_reason_wins_and_a_frame_with_no_calls_is_refused() {
        let f = frame(json!({
            "calls": [{"id": "call_1", "name": "terminal"}],
            "reason": "no git checkout under /opt/data/workbench/jobs/x",
        }));
        assert_eq!(
            step_of(&f, None).unrestorable_reason.as_deref(),
            Some("no git checkout under /opt/data/workbench/jobs/x")
        );
        for body in [
            json!({}),
            json!({"calls": []}),
            json!({"calls": [{"name": "terminal"}]}),
            json!({"calls": "terminal"}),
        ] {
            assert_eq!(read_frame(&body, 0), None, "{body}");
        }
    }

    #[test]
    fn ids_that_are_not_what_they_claim_are_dropped_rather_than_bound() {
        let f = frame(json!({
            "sessionId": "api-7f3a",
            "calls": [{"id": "call_1", "name": "terminal"}],
            "jobId": "not-a-job",
            "commit": "deadbeef",
            "tree": "0123456789abcdef0123456789abcdef0123456Z",
        }));
        assert_eq!(f.session_id, None);
        assert_eq!(f.job_id, None);
        assert_eq!(f.commit, None);
        assert_eq!(f.tree, None);
    }
}
