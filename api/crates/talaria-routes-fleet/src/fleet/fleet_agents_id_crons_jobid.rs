// /api/fleet/agents/{id}/crons/{jobId}. One cron job: DELETE → remove. POST
// { action } → pause | resume | run ("run" queues it for the next scheduler
// tick, ≤60s). PUT { name? schedule? prompt? } → edit in place. Admin or
// owner.

use super::can_manage_agent;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_crons::{
    edit_cron_job, pause_cron_job, remove_cron_job, resume_cron_job, run_cron_job,
};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{enum_member, optional_max_string_member, parse};
use talaria_error::{house_error, object_or_400};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;
pub async fn delete(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !can_manage_agent(&state, &user.id, &user.role, &id).await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
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
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
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
    let user = require_user(&state, &headers).await?;
    if !can_manage_agent(&state, &user.id, &user.role, &id).await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
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
    let prompt = match optional_max_string_member(obj, "prompt", 20_000) {
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
            Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
        },
    )
}

pub async fn post(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !can_manage_agent(&state, &user.id, &user.role, &id).await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let action = match enum_member(obj, "action", &["pause", "resume", "run"]) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
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
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    })
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
