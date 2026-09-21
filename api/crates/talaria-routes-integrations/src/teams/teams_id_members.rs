// /api/teams/{id}/members. GET → members (any member of the team).
// POST { email, role? } → add (owner; the role defaults to 'member', and the
// email rides the audit row exactly as sent). DELETE { userId } → remove
// (owner; owners are silently kept by the SQL's role guard). Non-uuid {id} →
// the house 500. Gate order: uuid bind, then the role check, then the body.

use super::owner_gate;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{email_member, enum_member, parse, uuid_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_session::{actor_of, require_user};
use talaria_state::AppState;
use talaria_teams::{add_team_member, list_team_members, remove_team_member};

const ROLES: &[&str] = &["owner", "member"];
pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("teams", "GET members", &id) {
        return Ok(gate);
    }
    if let Some(gate) = super::reader_gate(&state, &headers, &user.id, &id, "GET members").await {
        return Ok(gate);
    }
    Ok(match list_team_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members })).into_response(),
        Err(e) => internal("[teams] member list failed", e),
    })
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = talaria_params::uuid_gate("teams", "POST members", &id) {
        return Ok(gate);
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "POST members").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let email = match email_member(obj, "email") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // role's `.default('member')`: absent means member; present — including
    // null — must be one of ROLES, else the enum's message.
    let role = match obj.get("role") {
        None => "member".to_string(),
        Some(_) => match enum_member(obj, "role", ROLES) {
            Ok(v) => v,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
    };
    match add_team_member(&state.pg, &id, &email, &role).await {
        Ok(None) => {}
        Ok(Some(sentence)) => return Ok(house_error(StatusCode::BAD_REQUEST, &sentence)),
        Err(e) => return Ok(internal("[teams] member add failed", e)),
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.member_add",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(json!({ "email": email })),
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
    if let Some(gate) = talaria_params::uuid_gate("teams", "DELETE members", &id) {
        return Ok(gate);
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE members").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let user_id = match uuid_member(obj, "userId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(e) = remove_team_member(&state.pg, &id, &user_id).await {
        return Ok(internal("[teams] member remove failed", e));
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.member_remove",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(json!({ "userId": user_id })),
        },
    )
    .await;
    Ok(Json(json!({ "ok": true })).into_response())
}
