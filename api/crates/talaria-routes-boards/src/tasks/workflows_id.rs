// /api/workflows/{id}. PUT patch and DELETE, both agents.manage. Gate
// order: perm, body, then the id — so a member with a bad body gets the 403,
// a non-uuid id the house 400 ("not a uuid"), and a well-formed id that
// names no workflow a 404 on the PUT, an empty patch included: a write to
// nothing is not an ok. The DELETE of a missed id is the same 404.

use super::workflows::{match_json, toolkits_json, validate_workflow_body};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use talaria_body::parse;
use talaria_error::{house_error, internal, object_or_400};
use talaria_params::uuid_gate;
use talaria_session::require_perm;
use talaria_state::AppState;
use talaria_workflows::{WorkflowPatch, delete_workflow, update_workflow};

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let body = match validate_workflow_body(obj, false) {
        Ok(b) => b,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let patch = WorkflowPatch {
        name: body.name,
        description: body.description,
        enabled: body.enabled,
        match_v: match_json(&body.rules),
        skills: body.skills.as_ref().map(|v| json!(v)),
        toolkits: toolkits_json(&body.toolkits),
    };
    // The workflow is looked up even for an empty patch, so the id is
    // checked before every one.
    if let Some(gate) = uuid_gate("workflows", "PUT", &id) {
        return Ok(gate);
    }
    match update_workflow(&state.pg, &id, &patch).await {
        Ok(true) => Ok(Json(json!({ "ok": true })).into_response()),
        Ok(false) => Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => Ok(internal("[workflows] update failed", e)),
    }
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    if let Some(gate) = uuid_gate("workflows", "DELETE", &id) {
        return Ok(gate);
    }
    match delete_workflow(&state.pg, &id).await {
        Ok(true) => Ok(Json(json!({ "ok": true })).into_response()),
        Ok(false) => Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => Ok(internal("[workflows] delete failed", e)),
    }
}
