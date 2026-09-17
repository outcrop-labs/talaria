// /api/teams/{id}/agents. GET → agent members (any team member).
// POST { model } → add (owner). DELETE { model } → remove (owner).

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::session::{actor_of, require_user};
use crate::state::AppState;
use crate::teams::{add_team_agent, list_team_agents, remove_team_agent, team_role};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

fn uuid_gate(id: &str, action: &str) -> Option<Response> {
    if Uuid::parse_str(id).is_ok() {
        return None;
    }
    tracing::error!("[teams] non-uuid id on {action}: {id:?}");
    Some(thrown_internal_error())
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
    if let Some(gate) = uuid_gate(&id, "GET agents") {
        return gate;
    }
    if let Some(gate) = super::reader_gate(&state, &headers, &user.id, &id, "GET agents").await {
        return gate;
    }
    match list_team_agents(&state.pg, &id).await {
        Ok(agents) => Json(json!({ "agents": agents })).into_response(),
        Err(e) => {
            tracing::error!("[teams] agent list failed: {e}");
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
    if let Some(gate) = uuid_gate(&id, "POST agents") {
        return gate;
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "POST agents").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let model = match string_member(obj, "model", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match add_team_agent(&state.pg, &id, &model).await {
        Ok(None) => {}
        Ok(Some(sentence)) => return house_error(StatusCode::BAD_REQUEST, &sentence),
        Err(e) => {
            tracing::error!("[teams] agent add failed: {e}");
            return thrown_internal_error();
        }
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
    if let Some(gate) = uuid_gate(&id, "DELETE agents") {
        return gate;
    }
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE agents").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let model = match string_member(obj, "model", 1, 200) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = remove_team_agent(&state.pg, &id, &model).await {
        tracing::error!("[teams] agent remove failed: {e}");
        return thrown_internal_error();
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
    Json(json!({ "ok": true })).into_response()
}
