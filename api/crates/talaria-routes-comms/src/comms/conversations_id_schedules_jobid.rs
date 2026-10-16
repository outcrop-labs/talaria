// /api/conversations/{id}/schedules/{jobId}.
// DELETE → take a thread's schedule away: the job leaves the agent's own
// scheduler and the row that made this thread its listener goes with it. The
// same two gates as creating one — seeing the thread (404 for one you cannot
// reach), then the agent's managers (403), because stopping a timer is as much
// the agent's business as starting one. The row is removed even when the
// container cannot be reached: a row pointing at a job that no longer fires
// would list forever with nothing to show, and the person asked for exactly
// this removal.
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_crons::unschedule_from_conversation;
use talaria_audit::{AuditEntry, log_audit};
use talaria_conversations::accessible_conversation;
use talaria_error::{house_error, internal};
use talaria_session::{actor_of, require_agent_manager_by_model, require_user};
use talaria_state::AppState;

// doc: Remove a schedule this conversation created: the job and its listener row.
pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, job_id)): Path<(String, String)>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if talaria_params::uuid_gate_404(&id).is_some() {
        return Ok(house_error(StatusCode::NOT_FOUND, "conversation not found"));
    }
    let conv = match accessible_conversation(&state.pg, &user.id, &id).await {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "conversation not found")),
        Err(e) => {
            return Ok(internal(
                "[conversations] access read on unschedule failed",
                e,
            ));
        }
    };
    let user = match require_agent_manager_by_model(&state, &headers, &conv.agent_model).await {
        Ok(u) => u,
        Err(res) if res.status() == StatusCode::FORBIDDEN => {
            return Ok(house_error(
                StatusCode::FORBIDDEN,
                "only this agent's managers can unschedule it",
            ));
        }
        Err(res) => return Err(res),
    };
    if let Err(msg) = unschedule_from_conversation(&state.pg, &id, &job_id).await {
        // "no such schedule" is the one refusal that is the caller's state
        // being stale; everything else is the crate's own sentence.
        let status = if msg.contains("no such schedule") {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::BAD_REQUEST
        };
        return Ok(house_error(status, &msg));
    }
    let actor = actor_of(&user);
    let pg = state.pg.clone();
    let conversation = id.clone();
    let after = json!({ "jobId": job_id, "agentModel": conv.agent_model });
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: "cron.remove",
                target_type: "conversation",
                target_id: Some(&conversation),
                target_label: None,
                before: Some(after),
                after: None,
            },
        )
        .await;
    });
    Ok(Json(json!({ "ok": true })).into_response())
}
