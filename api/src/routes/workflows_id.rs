// /api/workflows/{id} — port of ui/src/routes/api/workflows.$id.ts. PUT
// patch and DELETE, both agents.manage. Gate order is TS's: perm, body, then
// the SQL bind — so a member with a bad body gets the 403, and a non-uuid id
// only reaches the bind when a field is PRESENT (an empty patch runs no SQL
// and answers ok even for a non-uuid id). No 404: a missed id updates or
// deletes nothing and the route still answers ok.

use super::workflows::{match_json, toolkits_json, validate_workflow_body};
use crate::body::{as_object, parse};
use crate::error::{house_error, thrown_internal_error};
use crate::params::uuid_gate;
use crate::session::require_perm;
use crate::state::AppState;
use crate::workflows::{WorkflowPatch, delete_workflow, update_workflow};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let body = match validate_workflow_body(obj, false) {
        Ok(b) => b,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let patch = WorkflowPatch {
        name: body.name,
        description: body.description,
        enabled: body.enabled,
        match_v: match_json(&body.rules),
        skills: body.skills.as_ref().map(|v| json!(v)),
        toolkits: toolkits_json(&body.toolkits),
    };
    // The uuid bind only happens when a statement runs — an empty patch
    // never reaches SQL, so it answers ok for any id spelling.
    let runs_sql = patch.name.is_some()
        || patch.description.is_some()
        || patch.enabled.is_some()
        || patch.match_v.is_some()
        || patch.skills.is_some()
        || patch.toolkits.is_some();
    if runs_sql && let Some(gate) = uuid_gate("workflows", "PUT", &id) {
        return gate;
    }
    if let Err(e) = update_workflow(&state.pg, &id, &patch).await {
        tracing::error!("[workflows] update failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    if let Some(gate) = uuid_gate("workflows", "DELETE", &id) {
        return gate;
    }
    if let Err(e) = delete_workflow(&state.pg, &id).await {
        tracing::error!("[workflows] delete failed: {e}");
        return thrown_internal_error();
    }
    Json(json!({ "ok": true })).into_response()
}
