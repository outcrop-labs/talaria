// /api/teams/{id}/agents. GET → agent members (any team member).
// POST { model } → add (owner). DELETE { model } → remove (owner).

use super::owner_gate;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;
use talaria_teams::{add_team_agent, list_team_agents, remove_team_agent};
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("teams", "GET agents", &id) {
        return Ok(gate);
    }
    if let Some(gate) = super::reader_gate(&state, &headers, &user.id, &id, "GET agents").await {
        return Ok(gate);
    }
    Ok(match list_team_agents(&state.pg, &id).await {
        Ok(agents) => Json(json!({ "agents": agents })).into_response(),
        Err(e) => internal("[teams] agent list failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("teams", "POST agents", &id) {
        return Ok(gate);
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "POST agents").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let model = match string_member(obj, "model", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match add_team_agent(&state.pg, &id, &model).await {
        Ok(None) => {}
        Ok(Some(sentence)) => return Ok(house_error(StatusCode::BAD_REQUEST, &sentence)),
        Err(e) => return Ok(internal("[teams] agent add failed", e)),
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.agent_add",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(json!({ "model": model })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("teams", "DELETE agents", &id) {
        return Ok(gate);
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE agents").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let model = match string_member(obj, "model", 1, 200) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(e) = remove_team_agent(&state.pg, &id, &model).await {
        return Ok(internal("[teams] agent remove failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.agent_remove",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(json!({ "model": model })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}
