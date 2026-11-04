// /api/workbench/flow. Per-repo git flow (the branch PRs target), across
// every registered git host. GET → configured flows + each host's reachable
// pool (a host that cannot list says why); PUT → set one repo's flow, on one
// host. agents.manage.

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_body::{nullish_max_string_member, parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts as hosts;
use talaria_session::require_perm;
use talaria_state::AppState;

pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let sb = state.secretbox().await.unwrap_or_default();
    let flows = match hosts::list_repo_flows(&state.pg).await {
        Ok(f) => f,
        Err(e) => return Ok(internal("[workbench/flow] flow read failed", e)),
    };
    let mut repos: Vec<Value> = Vec::new();
    let mut host_list: Vec<Value> = Vec::new();
    for (row, reached) in hosts::reachable_pool(&state.pg, &sb).await {
        let error = reached.as_ref().err().cloned();
        for repo in reached.unwrap_or_default() {
            repos.push(json!({ "host": row.id, "repo": repo }));
        }
        host_list.push(
            json!({ "id": row.id, "name": row.name, "kind": row.kind.as_str(), "error": error }),
        );
    }
    Ok(Json(json!({ "flows": flows, "repos": repos, "hosts": host_list })).into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    require_perm(&state, &headers, "agents.manage").await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let host = match string_member(obj, "host", 1, 40) {
        Ok(h) => h,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let repo = match string_member(obj, "repo", 3, 200) {
        Ok(r) => r,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    match hosts::get_host(&state.pg, &host).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::BAD_REQUEST, "no such git host")),
        Err(e) => return Ok(internal("[workbench/flow] host read failed", e)),
    }
    // Max-100 nullable-optional string, then trim-or-null: absent, null, and
    // blank-after-trim ALL land as null, so the PUT always sets the column.
    let read = |key: &str| nullish_max_string_member(obj, key, 100);
    let norm = |v: Option<String>| Some(v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
    let base = match read("baseBranch") {
        Ok(v) => norm(v),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Err(e) = hosts::set_repo_flow(&state.pg, &host, &repo, base).await {
        return Ok(internal("[workbench/flow] flow write failed", e));
    }
    let flows = match hosts::list_repo_flows(&state.pg).await {
        Ok(f) => f,
        Err(e) => return Ok(internal("[workbench/flow] flow read failed", e)),
    };
    Ok(Json(json!({ "flows": flows })).into_response())
}
