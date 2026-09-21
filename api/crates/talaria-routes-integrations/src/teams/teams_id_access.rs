// /api/teams/{id}/access. GET/PUT the team's platform views and permission
// overrides — admin-only, same privilege as Admin → People. A team grant
// expands to its human members at resolution time.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};
use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{as_object, optional_string_array_member, parse, zod_type_name};
use talaria_error::{house_error, thrown_internal_error};
use talaria_permissions::PERM_IDS;
use talaria_session::{actor_of, require_admin};
use talaria_state::AppState;
use talaria_teams::{
    get_team, get_team_access, set_team_allowed_manage_views, set_team_denied_views,
    set_team_perm_override,
};
use talaria_users::MANAGE_VIEW_ROUTES;
use uuid::Uuid;

fn uuid_gate(id: &str, action: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    tracing::error!("[teams] non-uuid id on {action}: {id:?}");
    Some(thrown_internal_error())
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(gate) = require_admin(&state, &headers).await {
        return gate;
    }
    if let Some(gate) = uuid_gate(&id, "GET access") {
        return gate;
    }
    match get_team(&state.pg, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[teams] access get team failed: {e}");
            return thrown_internal_error();
        }
    }
    match get_team_access(&state.pg, &id).await {
        Ok(access) => Json(json!({ "access": access })).into_response(),
        Err(e) => {
            tracing::error!("[teams] access read failed: {e}");
            thrown_internal_error()
        }
    }
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate(&id, "PUT access") {
        return gate;
    }
    match get_team(&state.pg, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[teams] access put team failed: {e}");
            return thrown_internal_error();
        }
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Some(denied) = match optional_string_array_member(obj, "deniedViews", 1, 60, 40) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    } {
        match set_team_denied_views(&state.pg, &id, &denied).await {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("[teams] denied views write failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    if let Some(allowed) = match optional_string_array_member(obj, "allowedManageViews", 1, 60, 10)
    {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    } {
        let valid: Vec<String> = allowed
            .into_iter()
            .filter(|v| MANAGE_VIEW_ROUTES.iter().any(|r| r == v))
            .collect();
        if let Err(e) = set_team_allowed_manage_views(&state.pg, &id, &valid).await {
            tracing::error!("[teams] manage views write failed: {e}");
            return thrown_internal_error();
        }
    }
    if let Some(perms) = obj.get("permissions") {
        let map = match perms {
            Value::Object(m) => m,
            other => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    &talaria_body::object_msg(zod_type_name(other)),
                );
            }
        };
        if let Err(gate) = apply_perm_overrides(&state, &id, map).await {
            return gate;
        }
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(&user),
            action: "team.access",
            target_type: "team",
            target_id: Some(&id),
            target_label: None,
            before: None,
            after: Some(parsed.clone()),
        },
    )
    .await;
    match get_team_access(&state.pg, &id).await {
        Ok(access) => Json(json!({ "access": access })).into_response(),
        Err(e) => {
            tracing::error!("[teams] access re-read failed: {e}");
            thrown_internal_error()
        }
    }
}

async fn apply_perm_overrides(
    state: &AppState,
    team_id: &str,
    map: &Map<String, Value>,
) -> Result<(), Response> {
    for (perm, val) in map {
        if !PERM_IDS.contains(&perm.as_str()) {
            return Err(house_error(
                StatusCode::BAD_REQUEST,
                &format!("unknown permission: {perm}"),
            ));
        }
        let allowed = match val {
            Value::Null => None,
            Value::Bool(b) => Some(*b),
            other => {
                return Err(house_error(
                    StatusCode::BAD_REQUEST,
                    &talaria_body::boolean_msg(zod_type_name(other)),
                ));
            }
        };
        if let Err(e) = set_team_perm_override(&state.pg, team_id, perm, allowed).await {
            tracing::error!("[teams] perm override write failed: {e}");
            return Err(thrown_internal_error());
        }
    }
    Ok(())
}
