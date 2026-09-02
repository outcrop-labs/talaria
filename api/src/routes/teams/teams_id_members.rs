// /api/teams/{id}/members. GET → members (any member of the team).
// POST { email, role? } → add (owner; the role defaults to 'member', and the
// email rides the audit row exactly as sent). DELETE { userId } → remove
// (owner; owners are silently kept by the SQL's role guard). Non-uuid {id} →
// the house 500. Gate order: uuid bind, then the role check, then the body.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, email_member, enum_member, parse, uuid_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::teams::{add_team_member, list_team_members, remove_team_member, team_role};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

const ROLES: &[&str] = &["owner", "member"];

fn uuid_gate(id: &str, action: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    tracing::error!("[teams] non-uuid id on {action}: {id:?}");
    Some(thrown_internal_error())
}

/// The member gate: any role on the team passes (None = proceed).
async fn member_gate(
    state: &AppState,
    user_id: &str,
    team_id: &str,
    action: &str,
) -> Option<Response> {
    match team_role(&state.pg, user_id, team_id).await {
        Ok(Some(_)) => None,
        Ok(None) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[teams] role read on {action} failed: {e}");
            Some(thrown_internal_error())
        }
    }
}

/// The owner gate: PATCH/DELETE-grade (None = proceed).
async fn owner_gate(
    state: &AppState,
    user_id: &str,
    team_id: &str,
    action: &str,
) -> Option<Response> {
    match team_role(&state.pg, user_id, team_id).await {
        Ok(Some(role)) if role == "owner" => None,
        Ok(_) => Some(house_error(StatusCode::FORBIDDEN, "forbidden")),
        Err(e) => {
            tracing::error!("[teams] role read on {action} failed: {e}");
            Some(thrown_internal_error())
        }
    }
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate(&id, "GET members") {
        return gate;
    }
    if let Some(gate) = member_gate(&state, &user.id, &id, "GET members").await {
        return gate;
    }
    match list_team_members(&state.pg, &id).await {
        Ok(members) => Json(json!({ "members": members })).into_response(),
        Err(e) => {
            tracing::error!("[teams] member list failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate(&id, "POST members") {
        return gate;
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "POST members").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let email = match email_member(obj, "email") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // role's `.default('member')`: absent means member; present — including
    // null — must be one of ROLES, else the enum's message.
    let role = match obj.get("role") {
        None => "member".to_string(),
        Some(_) => match enum_member(obj, "role", ROLES) {
            Ok(v) => v,
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        },
    };
    match add_team_member(&state.pg, &id, &email, &role).await {
        Ok(None) => {}
        Ok(Some(sentence)) => return house_error(StatusCode::BAD_REQUEST, &sentence),
        Err(e) => {
            tracing::error!("[teams] member add failed: {e}");
            return thrown_internal_error();
        }
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
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate(&id, "DELETE members") {
        return gate;
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE members").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let user_id = match uuid_member(obj, "userId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = remove_team_member(&state.pg, &id, &user_id).await {
        tracing::error!("[teams] member remove failed: {e}");
        return thrown_internal_error();
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
    Json(json!({ "ok": true })).into_response()
}
