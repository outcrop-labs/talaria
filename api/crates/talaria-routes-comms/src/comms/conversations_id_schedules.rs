// /api/conversations/{id}/schedules.
// GET → what this thread has put its agent on a timer for. Two halves: the
// rows Talaria wrote when the schedule was created (name, schedule, when,
// which job), and the live state only the agent's own jobs.json knows — is it
// still ticking, when does it run next, how did the last run end.
//
// A CONTAINER THAT CANNOT BE READ IS NOT A FAILED LIST. The rows are still
// the right answer to "what did I schedule here", so the live fields come
// back null and the list renders; failing the whole read because an agent is
// down would hide the schedules exactly when somebody is trying to work out
// why nothing has posted.
//
// READING ONLY needs access to the thread — creating one needs the agent's
// managers (see the POST next door), because that is what spends.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_agent_crons::{live_jobs_for_model, schedules_for_conversation};
use talaria_conversations::accessible_conversation;
use talaria_error::{house_error, internal};
use talaria_session::require_user;
use talaria_state::AppState;

// doc: The cron jobs this conversation has scheduled on its agent, each with
// doc: the live job state when the agent's container can be read.

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    // The same 404 the POST answers, for the same reason.
    if talaria_params::uuid_gate_404(&id).is_some() {
        return Ok(house_error(StatusCode::NOT_FOUND, "conversation not found"));
    }
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "conversation not found")),
        Err(e) => {
            return Ok(internal(
                "[conversations] access read on schedules failed",
                e,
            ));
        }
    };
    let rows = match schedules_for_conversation(&state.pg, &id).await {
        Ok(rows) => rows,
        Err(e) => return Ok(internal("[conversations] schedule read failed", e)),
    };
    // Most threads have scheduled nothing, and asking a container for its
    // jobs.json is a docker exec — so the live half is only fetched when
    // there is a row to merge it onto.
    let live = if rows.is_empty() {
        Vec::new()
    } else {
        match live_jobs_for_model(&state.pg, &conv.agent_model).await {
            Ok(jobs) => jobs,
            Err(e) => {
                tracing::warn!("[conversations] live cron read for {id} failed: {e}");
                Vec::new()
            }
        }
    };
    let schedules: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            // The row's half of the answer is named by ScheduleRow's own
            // serde, so the two halves can never disagree about a key; the
            // live fields are inserted on top of it. A job the container does
            // not list — removed through the agent's own cron list, or an
            // unreadable container — leaves every one of them null.
            let mut value = serde_json::to_value(row).unwrap_or_else(|_| json!({}));
            let job = live.iter().find(|j| j.id == row.job_id);
            if let Some(obj) = value.as_object_mut() {
                obj.insert("enabled".into(), json!(job.map(|j| j.enabled)));
                obj.insert("state".into(), json!(job.map(|j| j.state.clone())));
                obj.insert(
                    "nextRunAt".into(),
                    json!(job.and_then(|j| j.next_run_at.clone())),
                );
                obj.insert(
                    "lastRunAt".into(),
                    json!(job.and_then(|j| j.last_run_at.clone())),
                );
                obj.insert(
                    "lastStatus".into(),
                    json!(job.and_then(|j| j.last_status.clone())),
                );
            }
            value
        })
        .collect();
    Ok(Json(json!({ "schedules": schedules })).into_response())
}
