// /api/fleet/agents/{id}/crons/{jobId}. One cron job: DELETE → remove. POST
// { action } → pause | resume | run ("run" queues it for the next scheduler
// tick, ≤60s; a paused job is refused, because Hermes's run would resume it
// for good). PUT { name? schedule? prompt? } → edit in place. Admin or
// owner.
//
// A change the agent took and never answered is a 503, not a refusal
// ([`cron_failure`]): the agent may still apply it, and its sentence says so.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_crons::{
    edit_cron_job, may_still_land, pause_cron_job, remove_cron_job, resume_cron_job, run_cron_job,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{enum_member, optional_max_string_member, parse};
use talaria_error::{house_error, object_or_400, store_answer};
use talaria_session::{actor_of, require_agent_manager};
use talaria_state::AppState;
pub async fn delete(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    Ok(match remove_cron_job(&state.pg, &id, &job_id).await {
        Ok(()) => {
            audit(
                &state,
                &user,
                "cron.delete",
                &id,
                json!({ "jobId": job_id }),
            );
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => cron_failure(&e),
    })
}

/// PUT { name? schedule? prompt? } — any subset; members are untrimmed
/// (min 1 / max n on the raw string).
pub async fn put(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match optional_max_string_member(obj, "name", 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(n) = &name
        && n.is_empty()
    {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &talaria_body::too_small_msg(1),
        ));
    }
    let schedule = match optional_max_string_member(obj, "schedule", 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(s) = &schedule
        && s.is_empty()
    {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &talaria_body::too_small_msg(1),
        ));
    }
    // The agent's own cap: Hermes's /api/jobs refuses a longer prompt
    // (talaria_agent_crons::MAX_PROMPT_CHARS).
    let prompt = match optional_max_string_member(obj, "prompt", 5_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(p) = &prompt
        && p.is_empty()
    {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &talaria_body::too_small_msg(1),
        ));
    }
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    Ok(
        match edit_cron_job(
            &state.pg,
            &id,
            &job_id,
            name.as_deref(),
            schedule.as_deref(),
            prompt.as_deref(),
        )
        .await
        {
            Ok(()) => {
                audit(
                    &state,
                    &user,
                    "cron.update",
                    &id,
                    json!({ "jobId": job_id, "name": name, "schedule": schedule }),
                );
                Json(json!({ "ok": true })).into_response()
            }
            Err(e) => cron_failure(&e),
        },
    )
}

pub async fn post(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_agent_manager(&state, &headers, &id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let action = match enum_member(obj, "action", &["pause", "resume", "run"]) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(gate) = talaria_params::uuid_gate_404(&id) {
        return Ok(gate);
    }
    let result = match action.as_str() {
        "pause" => pause_cron_job(&state.pg, &id, &job_id).await,
        "resume" => resume_cron_job(&state.pg, &id, &job_id).await,
        _ => run_cron_job(&state.pg, &id, &job_id).await,
    };
    Ok(match result {
        Ok(()) => {
            audit(
                &state,
                &user,
                &format!("cron.{action}"),
                &id,
                json!({ "jobId": job_id }),
            );
            Json(json!({ "ok": true })).into_response()
        }
        Err(e) => cron_failure(&e),
    })
}

/// A cron call's failure as the routes answer it. A change the agent took and
/// did not answer within its window (`talaria_agent_crons::may_still_land`)
/// was not refused: under a stalled disk a pause answered "cannot reach"
/// landed ten minutes later. That is a 503, the house's "not confirmed" (the
/// control route's for a docker verb past its deadline), with the sentence
/// that says the change may still land. Every other failure — Hermes's own
/// refusal, a bad id, a paused job's run, an agent never reached — is the
/// 400 it always was.
pub(crate) fn cron_failure(e: &str) -> Response {
    if may_still_land(e) {
        return house_error(StatusCode::SERVICE_UNAVAILABLE, e);
    }
    store_answer(StatusCode::BAD_REQUEST, e)
}

/// The family's audit shape — names only, never values, fire-and-forget.
fn audit(
    state: &AppState,
    user: &talaria_session::SessionUser,
    action: &str,
    id: &str,
    after: Value,
) {
    let actor = actor_of(user);
    let action = action.to_string();
    let id = id.to_string();
    let pg = state.pg.clone();
    tokio::spawn(async move {
        let after = after;
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: &action,
                target_type: "agent",
                target_id: Some(&id),
                target_label: None,
                before: None,
                after: Some(after),
            },
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn answered(r: Response) -> (StatusCode, serde_json::Value) {
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 16).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    /// FLEET-5: a pause, run or resume the agent took and never answered was
    /// a 400 "cannot reach … no answer within 40s" — and the pause landed. It
    /// is a 503 now, with the sentence that says the change may still land;
    /// a refusal keeps its 400.
    #[tokio::test]
    async fn a_change_the_agent_never_answered_is_not_a_refusal() {
        let unconfirmed = "ada-ops's scheduler did not answer within 40s, so the change may still land — check the job list in a minute before trying again";
        let (status, body) = answered(cron_failure(unconfirmed)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["error"], unconfirmed);

        for refusal in [
            "Job not found",
            "this job is paused — resume it first; running a paused job would resume it for good",
            "cannot reach ada-ops's scheduler: no answer within 40s",
            "cannot reach ada-ops's scheduler: not in the fleet manifest",
        ] {
            let (status, body) = answered(cron_failure(refusal)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{refusal}");
            assert_eq!(body["error"], refusal);
        }
    }
}
