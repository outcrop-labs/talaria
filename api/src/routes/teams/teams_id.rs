// /api/teams/{id}. GET → team + members + agents (member, or Manage → Teams).
// PATCH { name?, description? } → rename / set blurb (owner); DELETE → delete
// (owner) — the member rows cascade and its boards survive as personal boards
// (team_id set null, not cascaded), which is why both are owner-gated. A
// non-uuid {id} → the house 500. Gate order: uuid bind, then the owner
// check, then the body — a non-owner with a bad body gets the 403.

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, present_nullable_string_member, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::teams::{
    delete_team, get_team, list_team_agents, list_team_members, rename_team, set_team_description,
    team_role,
};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

fn uuid_gate(id: &str, action: &str) -> Option<Response> {
    crate::params::uuid_gate("teams", action, id)
}

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
    if let Some(gate) = uuid_gate(&id, "GET") {
        return gate;
    }
    if let Some(gate) = super::reader_gate(&state, &headers, &user.id, &id, "GET").await {
        return gate;
    }
    let role = match team_role(&state.pg, &user.id, &id).await {
        Ok(r) => r.unwrap_or_default(),
        Err(e) => {
            tracing::error!("[teams] role read on GET failed: {e}");
            return thrown_internal_error();
        }
    };
    let row = match get_team(&state.pg, &id).await {
        Ok(Some(r)) => r,
        Ok(None) => return house_error(StatusCode::NOT_FOUND, "not found"),
        Err(e) => {
            tracing::error!("[teams] get failed: {e}");
            return thrown_internal_error();
        }
    };
    let members = match list_team_members(&state.pg, &id).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[teams] member list on GET failed: {e}");
            return thrown_internal_error();
        }
    };
    let agents = match list_team_agents(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[teams] agent list on GET failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({
        "team": {
            "id": row.0,
            "name": row.1,
            "description": row.2,
            "createdAt": crate::agent_auth::epoch_ms_to_iso(row.3),
            "role": role,
            "memberCount": members.len() as i32,
            "agentCount": agents.len() as i32,
        },
        "members": members,
        "agents": agents,
    }))
    .into_response()
}

pub async fn patch(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate(&id, "PATCH") {
        return gate;
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "PATCH").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let name = match obj.get("name") {
        None => None,
        Some(_) => match string_member(obj, "name", 1, 120) {
            Ok(v) => Some(v),
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        },
    };
    let description = match present_nullable_string_member(obj, "description", 500) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if name.is_none() && description.is_none() {
        return house_error(StatusCode::BAD_REQUEST, "nothing to update");
    }
    if let Some(name) = &name {
        match rename_team(&state.pg, &id, name).await {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("[teams] rename failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    if let Some(desc) = &description {
        match set_team_description(&state.pg, &id, desc.as_deref()).await {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("[teams] description write failed: {e}");
                return thrown_internal_error();
            }
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
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let user = match require_user(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    if let Some(gate) = uuid_gate(&id, "DELETE") {
        return gate;
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE").await {
        return gate;
    }
    if let Err(e) = delete_team(&state.pg, &id).await {
        tracing::error!("[teams] delete failed: {e}");
        return thrown_internal_error();
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
    Json(json!({ "ok": true })).into_response()
}
