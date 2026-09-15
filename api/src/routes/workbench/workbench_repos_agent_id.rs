// /api/workbench/repos/{agentId}. Per-agent workbench repo grants —
// explicit, like MCP assignment. GET → the connection's reachable pool +
// this agent's grants; PUT → replace the grant set (validated against the
// pool). agents.manage.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::body::{as_object, parse, string_array_member};
use crate::error::{house_error, thrown_internal_error};
use crate::github as gh;
use crate::session::require_perm;
use crate::state::AppState;

/// A non-uuid id folds into "no such agent", never a 500.
async fn agent_exists(pg: &sqlx::PgPool, id: &str) -> bool {
    sqlx::query_scalar::<_, i32>("select 1 from agent_defs where id = $1::uuid")
        .bind(id)
        .fetch_optional(pg)
        .await
        .map(|r| r.is_some())
        .unwrap_or(false)
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    if !agent_exists(&state.pg, &agent_id).await {
        return house_error(StatusCode::NOT_FOUND, "unknown agent");
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let available = gh::list_reachable_repos(&state.pg, &sb).await;
    let granted = gh::granted_repos(&state.pg, &agent_id).await;
    // The rules live beside the grants, and each granted repo carries its
    // live branch list for the pickers — the admin configures against what
    // actually exists, not a free-text guess.
    let rules = gh::repo_rules(&state.pg, &agent_id).await;
    let mut branches = serde_json::Map::new();
    for rule in &rules {
        branches.insert(
            rule.repo.clone(),
            json!(gh::list_branches(&state.pg, &sb, &rule.repo).await),
        );
    }
    Json(json!({
        "available": available,
        "granted": granted,
        "rules": rules,
        "branches": branches,
    }))
    .into_response()
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Response {
    if let Err(gate) = require_perm(&state, &headers, "agents.manage").await {
        return gate;
    }
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    // Required array (min 0 — an empty PUT clears the grants), elements ≤200,
    // ≤100 items. Validation runs before the agent check, the agent check
    // before the pool filter.
    let repos = match string_array_member(obj, "repos", 0, 200, 0, 100) {
        Ok(v) => v,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if !agent_exists(&state.pg, &agent_id).await {
        return house_error(StatusCode::NOT_FOUND, "unknown agent");
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let pool = gh::list_reachable_repos(&state.pg, &sb).await;
    // Silently dropped, not rejected: a repo that fell out of the connection
    // since the UI loaded isn't a user error.
    let repos: Vec<String> = repos.into_iter().filter(|r| pool.contains(r)).collect();
    if let Err(e) = gh::set_granted_repos(&state.pg, &agent_id, &repos).await {
        tracing::error!("[workbench/repos] grant write failed: {e}");
        return thrown_internal_error();
    }
    // Rules ride the same PUT, optional: an array of {repo, baseBranch?,
    // pushMode?, branchPrefix?}. Only rules for STILL-GRANTED repos are
    // honored — a rule for a repo the same request revoked is a UI race, and
    // the quiet drop is the honest resolution.
    if let Some(v) = obj.get("rules") {
        let arr = match v.as_array() {
            Some(a) => a,
            None => return house_error(StatusCode::BAD_REQUEST, "rules must be an array"),
        };
        for el in arr {
            let Some(entry) = el.as_object() else {
                return house_error(StatusCode::BAD_REQUEST, "rules entries must be objects");
            };
            let repo = match crate::body::string_member(entry, "repo", 3, 200) {
                Ok(r) => r,
                Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
            };
            if !repos.contains(&repo) {
                continue;
            }
            let rule = gh::RepoRule {
                repo,
                base_branch: crate::body::optional_max_string_member(entry, "baseBranch", 200)
                    .ok()
                    .flatten()
                    .filter(|b| !b.is_empty()),
                push_mode: {
                    match crate::body::enum_member(entry, "pushMode", &["branches_only", "free"]) {
                        Ok(m) => m.to_string(),
                        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
                    }
                },
                branch_prefix: crate::body::optional_max_string_member(entry, "branchPrefix", 100)
                    .ok()
                    .flatten()
                    .filter(|p| !p.is_empty()),
            };
            if let Err(e) = gh::set_repo_rule(&state.pg, &agent_id, &rule).await {
                tracing::error!("[workbench/repos] rule write failed: {e}");
                return thrown_internal_error();
            }
        }
    }
    Json(json!({ "granted": repos })).into_response()
}
