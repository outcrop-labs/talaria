// /api/research/{id}/teams.
// POST { teamId } → grant a team (run owner). DELETE { teamId } → revoke
// (run owner). Keeps the report artifact's editor grants in step when the
// report exists.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_api_facades::kb::perms::{EditorGrant, list_editors, set_editors};
use talaria_body::{parse, uuid_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_research::{
    add_research_team, remove_research_team, research_artifact_for, research_role,
};
use talaria_session::require_user;
use talaria_state::AppState;
use talaria_teams::get_team;

async fn sync_report_grant_team(
    state: &AppState,
    run_id: &str,
    team_id: &str,
    present: bool,
) -> Result<(), sqlx::Error> {
    let Some(artifact_id) = research_artifact_for(&state.pg, run_id).await? else {
        return Ok(());
    };
    let mut grants: Vec<EditorGrant> = list_editors(&state.pg, "artifact", &artifact_id)
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
    set_editors(&state.pg, "artifact", &artifact_id, &grants).await
}

async fn owner_gate(state: &AppState, user_id: &str, id: &str, action: &str) -> Option<Response> {
    if let Some(gate) = talaria_params::uuid_gate("research", action, id) {
        return Some(gate);
    }
    match research_role(&state.pg, Some(user_id), id).await {
        Ok(Some("owner")) => None,
        Ok(_) => Some(house_error(
            StatusCode::FORBIDDEN,
            "only the research owner can share it",
        )),
        Err(e) => Some(internal(
            &format!("[research] role read on {action} failed"),
            e,
        )),
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = owner_gate(&state, &user.id, &id, "POST teams").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let team_id = match uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match get_team(&state.pg, &team_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::BAD_REQUEST, "team not found")),
        Err(e) => return Ok(internal("[research] team lookup on grant failed", e)),
    }
    if let Err(e) = add_research_team(&state.pg, &id, &team_id).await {
        return Ok(internal("[research] team grant failed", e));
    }
    if let Err(e) = sync_report_grant_team(&state, &id, &team_id, true).await {
        tracing::error!("[research] report grant sync on team share failed: {e}");
    }
    Ok(Json(json!({ "ok": true })).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    let user = require_user(&state, &headers).await?;
    if let Some(gate) = owner_gate(&state, &user.id, &id, "DELETE teams").await {
        return Ok(gate);
    }
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let team_id = match uuid_member(obj, "teamId") {
        Ok(v) => v,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(e) = remove_research_team(&state.pg, &id, &team_id).await {
        return Ok(internal("[research] team revoke failed", e));
    }
    if let Err(e) = sync_report_grant_team(&state, &id, &team_id, false).await {
        tracing::error!("[research] report grant sync on team unshare failed: {e}");
    }
    Ok(Json(json!({ "ok": true })).into_response())
}
