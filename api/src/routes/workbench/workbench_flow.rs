// /api/workbench/flow — port of ui/src/routes/api/workbench.flow.ts.
// Per-repo git flow (PR base + optional testing branch). GET → configured
// flows + the reachable pool; PUT → set one repo's flow. agents.manage.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::body::{as_object, nullish_max_string_member, parse, string_member};
use crate::error::{house_error, thrown_internal_error};
use crate::github as gh;
use crate::session::require_perm;
use crate::state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let flows = match gh::list_repo_flows(&state.pg).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[workbench/flow] flow read failed: {e}");
            return thrown_internal_error();
        }
    };
    let repos = gh::list_reachable_repos(&state.pg, &sb).await;
    Json(json!({ "flows": flows, "repos": repos })).into_response()
}

pub async fn put(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let repo = match string_member(obj, "repo", 3, 200) {
        Ok(r) => r,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // z.string().max(100).nullable().optional() — then `?.trim() || null`:
    // absent, null, and blank-after-trim ALL land as null, so the PUT always
    // sets both columns (a PUT with only one field clears the other).
    let read = |key: &str| nullish_max_string_member(obj, key, 100);
    let norm = |v: Option<String>| Some(v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    let base = match read("baseBranch") {
        Ok(v) => norm(v),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let testing = match read("testingBranch") {
        Ok(v) => norm(v),
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = gh::set_repo_flow(&state.pg, &repo, base, testing).await {
        tracing::error!("[workbench/flow] flow write failed: {e}");
        return thrown_internal_error();
    }
    let flows = match gh::list_repo_flows(&state.pg).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[workbench/flow] flow read failed: {e}");
            return thrown_internal_error();
        }
    };
    Json(json!({ "flows": flows })).into_response()
}
