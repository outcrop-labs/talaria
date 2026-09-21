// /api/plans/{id}/teams.
// POST { teamId } → grant a team (owner). DELETE { teamId } → revoke (owner).
// Keeps the plan doc's editor grants in step with the team principal.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::kb::perms::{EditorGrant, list_editors, set_editors};
use talaria_body::{as_object, parse, uuid_member};
use talaria_conversations::{add_plan_team, plan_role, remove_plan_team};
use talaria_error::{house_error, thrown_internal_error};
use talaria_params::uuid_gate;
use talaria_plan_doc::plan_doc_for;
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_teams::get_team;

async fn sync_doc_grant_team(
    pg: &sqlx::PgPool,
    plan_id: &str,
    team_id: &str,
    present: bool,
) -> Result<(), sqlx::Error> {
    let Some(doc) = plan_doc_for(pg, plan_id).await? else {
        return Ok(());
    };
    let mut grants: Vec<EditorGrant> = list_editors(pg, "artifact", &doc.id)
        .await?
        .into_iter()
        .filter(|g| !(g.principal_type == "team" && g.principal_id == team_id))
        .collect();
    if present {
        grants.push(EditorGrant {
            principal_type: "team".into(),
            principal_id: team_id.to_string(),
            role: "editor".into(),
        });
    }
    set_editors(pg, "artifact", &doc.id, &grants).await
}

async fn owner_gate(state: &AppState, user_id: &str, id: &str, action: &str) -> Option<Response> {
    if let Some(gate) = uuid_gate("plans", action, id) {
        return Some(gate);
    }
    match plan_role(&state.pg, user_id, id).await {
        Ok(Some(role)) if role == "owner" => None,
        Ok(_) => Some(house_error(
            StatusCode::FORBIDDEN,
            "only the plan owner can share it",
        )),
        Err(e) => {
            tracing::error!("[plans] plan role read on {action} failed: {e}");
            Some(thrown_internal_error())
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
    if let Some(gate) = owner_gate(&state, &user.id, &id, "POST teams").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let team_id = match uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    match get_team(&state.pg, &team_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return house_error(StatusCode::BAD_REQUEST, "team not found"),
        Err(e) => {
            tracing::error!("[plans] team lookup on grant failed: {e}");
            return thrown_internal_error();
        }
    }
    if let Err(e) = add_plan_team(&state.pg, &id, &team_id).await {
        tracing::error!("[plans] team grant failed: {e}");
        return thrown_internal_error();
    }
    if let Err(e) = sync_doc_grant_team(&state.pg, &id, &team_id, true).await {
        tracing::error!("[plans] doc grant on team share failed: {e}");
        return thrown_internal_error();
    }
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
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE teams").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let team_id = match uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = remove_plan_team(&state.pg, &id, &team_id).await {
        tracing::error!("[plans] team revoke failed: {e}");
        return thrown_internal_error();
    }
    if let Err(e) = sync_doc_grant_team(&state.pg, &id, &team_id, false).await {
        tracing::error!("[plans] doc grant on team unshare failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}
