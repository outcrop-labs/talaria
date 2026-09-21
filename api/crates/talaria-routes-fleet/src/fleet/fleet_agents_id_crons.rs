// /api/fleet/agents/{id}/crons. One agent's native Hermes cron jobs. GET →
// jobs (read from the container's jobs.json). POST → create. Admin, or the
// owner of a personal assistant.

use super::can_manage_agent;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_agent_crons::{create_cron_job, list_cron_jobs};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, trimmed_string_member};
use talaria_error::{house_error, object_or_400};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;
pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !can_manage_agent(&state, &user.id, &user.role, &id).await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(match list_cron_jobs(&state.pg, &id).await {
        Ok(jobs) => Json(json!({ "jobs": jobs })).into_response(),
        Err(e) => house_error(StatusCode::BAD_REQUEST, &e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if !can_manage_agent(&state, &user.id, &user.role, &id).await {
        return Ok(house_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match trimmed_string_member(obj, "name", 1, 80) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let schedule = match trimmed_string_member(obj, "schedule", 1, 120) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let prompt = match trimmed_string_member(obj, "prompt", 1, 20_000) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    Ok(
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
        },
    )
}
