// /api/teams/{id}. GET → team + members + agents (member, or Manage → Teams).
// PATCH { name?, description? } → rename / set blurb (owner); DELETE → delete
// (owner) — the member rows cascade and its boards survive as personal boards
// (team_id set null, not cascaded), which is why both are owner-gated. A
// non-uuid {id} → the house 500. Gate order: uuid bind, then the owner
// check, then the body — a non-owner with a bad body gets the 403.

use super::owner_gate;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, present_nullable_string_member, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;
use talaria_teams::{
    delete_team, get_team, list_team_agents, list_team_members, rename_team, set_team_description,
    team_role,
};
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("teams", "GET", &id) {
        return Ok(gate);
    }
    if let Some(gate) = super::reader_gate(&state, &headers, &user.id, &id, "GET").await {
        return Ok(gate);
    }
    let role = match team_role(&state.pg, &user.id, &id).await {
        Ok(r) => r.unwrap_or_default(),
        Err(e) => return Ok(internal("[teams] role read on GET failed", e)),
    };
    let row = match get_team(&state.pg, &id).await {
        Ok(Some(r)) => r,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[teams] get failed", e)),
    };
    let members = match list_team_members(&state.pg, &id).await {
        Ok(m) => m,
        Err(e) => return Ok(internal("[teams] member list on GET failed", e)),
    };
    let agents = match list_team_agents(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => return Ok(internal("[teams] agent list on GET failed", e)),
    };
    Ok(Json(json!({
        "team": {
            "id": row.0,
            "name": row.1,
            "description": row.2,
            "createdAt": talaria_agent_auth::epoch_ms_to_iso(row.3),
            "role": role,
            "memberCount": members.len() as i32,
            "agentCount": agents.len() as i32,
        },
        "members": members,
        "agents": agents,
    }))
    .into_response())
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("teams", "PATCH", &id) {
        return Ok(gate);
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "PATCH").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let name = match obj.get("name") {
        None => None,
        Some(_) => match string_member(obj, "name", 1, 120) {
            Ok(v) => Some(v),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
    };
    let description = match present_nullable_string_member(obj, "description", 500) {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if name.is_none() && description.is_none() {
        return Ok(house_error(StatusCode::BAD_REQUEST, "nothing to update"));
    }
    if let Some(name) = &name {
        match rename_team(&state.pg, &id, name).await {
            Ok(()) => {}
            Err(e) => return Ok(internal("[teams] rename failed", e)),
        }
    }
    if let Some(desc) = &description {
        match set_team_description(&state.pg, &id, desc.as_deref()).await {
            Ok(()) => {}
            Err(e) => return Ok(internal("[teams] description write failed", e)),
        }
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.rename",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(json!({ "name": name, "description": description })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("teams", "DELETE", &id) {
        return Ok(gate);
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE").await {
        return Ok(gate);
    }
    if let Err(e) = delete_team(&state.pg, &id).await {
        return Ok(internal("[teams] delete failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.delete",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}
