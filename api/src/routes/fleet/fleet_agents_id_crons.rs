// /api/fleet/agents/{id}/crons. One agent's native Hermes cron jobs. GET →
// jobs (read from the container's jobs.json). POST → create. Admin, or the
// owner of a personal assistant.

use crate::agent_crons::{create_cron_job, list_cron_jobs};
use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, trimmed_string_member};
use crate::error::house_error;
use crate::permissions::has_perm;
use crate::personal_agent::owns_agent;
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// The shared gate: admin via agents.manage, or the owner of a personal
/// assistant. A permissions read that errors reads as NO — fail-closed is
/// the family's posture.
async fn gate(state: &AppState, user_id: &str, role: &str, id: &str) -> bool {
    match has_perm(&state.pg, user_id, role, "agents.manage").await {
        Ok(true) => true,
        Ok(false) => owns_agent(&state.pg, user_id, None, Some(id)).await,
        Err(_) => false,
    }
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if !gate(&state, &user.id, &user.role, &id).await {
        return house_error(StatusCode::FORBIDDEN, "forbidden");
    }
    match list_cron_jobs(&state.pg, &id).await {
        Ok(jobs) => Json(json!({ "jobs": jobs })).into_response(),
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}

pub async fn post(
    State(state): State<AppState>,
    Path(id): Path<String>,
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
    let name = match trimmed_string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let schedule = match trimmed_string_member(obj, "schedule", 1, 120) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let prompt = match trimmed_string_member(obj, "prompt", 1, 20_000) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match create_cron_job(&state.pg, &id, &name, &schedule, &prompt).await {
        Ok(created) => {
            let actor = actor_of(&user);
            let after = json!({ "name": name, "schedule": schedule });
            let id_for_audit = id.clone();
            let pg = state.pg.clone();
            tokio::spawn(async move {
                log_audit(
                    &pg,
                    AuditEntry {
                        actor: &actor,
                        action: "cron.create",
                        target_type: "agent",
                        target_id: Some(&id_for_audit),
                        target_label: None,
                        before: None,
                        after: Some(after),
                    },
                )
                .await;
            });
            // wire shape — { ok, id }, id after ok.
            Json(json!({ "ok": true, "id": created })).into_response()
        }
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    }
}
