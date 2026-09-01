// /api/fleet/agents/{id}/crons/{jobId} — port of
// ui/src/routes/api/fleet.agents.$id.crons.$jobId.ts. One cron job: DELETE →
// remove. POST { action } → pause | resume | run ("run" queues it for the
// next scheduler tick, ≤60s). PUT { name? schedule? prompt? } → edit in
// place. Admin or owner.

use crate::agent_crons::{
    edit_cron_job, pause_cron_job, remove_cron_job, resume_cron_job, run_cron_job,
};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, enum_member, optional_max_string_member, parse};
use crate::error::house_error;
use crate::permissions::has_perm;
use crate::personal_agent::owns_agent;
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// The family's gate (see fleet_agents_id_crons): admin via agents.manage, or
/// the owner of a personal assistant.
async fn gate(state: &AppState, user_id: &str, role: &str, id: &str) -> bool {
    match has_perm(&state.pg, user_id, role, "agents.manage").await {
        Ok(true) => true,
        Ok(false) => owns_agent(&state.pg, user_id, None, Some(id)).await,
        Err(_) => false,
    }
}

pub async fn delete(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !gate(&state, &user.id, &user.role, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match remove_cron_job(&state.pg, &id, &job_id).await {
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
    }
}

/// PUT { name? schedule? prompt? } — any subset; zod's members are
/// untrimmed (min 1 / max n on the raw string).
pub async fn put(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !gate(&state, &user.id, &user.role, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match optional_max_string_member(obj, "name", 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Some(n) = &name
        && n.is_empty()
    {
        return house_error(StatusCode::BAD_REQUEST, &crate::body::too_small_msg(1));
    }
    let schedule = match optional_max_string_member(obj, "schedule", 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Some(s) = &schedule
        && s.is_empty()
    {
        return house_error(StatusCode::BAD_REQUEST, &crate::body::too_small_msg(1));
    }
    let prompt = match optional_max_string_member(obj, "prompt", 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Some(p) = &prompt
        && p.is_empty()
    {
        return house_error(StatusCode::BAD_REQUEST, &crate::body::too_small_msg(1));
    }
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
    }
}

pub async fn post(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !gate(&state, &user.id, &user.role, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let action = match enum_member(obj, "action", &["pause", "resume", "run"]) {
        Ok(a) => a,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let result = match action.as_str() {
        "pause" => pause_cron_job(&state.pg, &id, &job_id).await,
        "resume" => resume_cron_job(&state.pg, &id, &job_id).await,
        _ => run_cron_job(&state.pg, &id, &job_id).await,
    };
    match result {
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
    }
}

/// The family's audit shape — names only, never values, fire-and-forget.
fn audit(
    state: &AppState,
    user: &crate::session::SessionUser,
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
