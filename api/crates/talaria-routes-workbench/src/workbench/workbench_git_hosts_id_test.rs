// /api/workbench/git-hosts/{id}/test. "Test connection": POST asks the host,
// live, who its connection acts as and which repositories it can reach — the
// two things a workbench job needs from it. Every call underneath is a read.
// requireAdmin, like the rest of the host plane.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use talaria_error::{house_error, internal};
use talaria_git_hosts::GitHost;
use talaria_git_hosts::registry;
use talaria_session::require_admin;
use talaria_state::AppState;

/// How many reachable repositories the answer names (the count is whole).
const SAMPLE: usize = 10;

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let row = match registry::get_host(&state.pg, &id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "no such git host")),
        Err(e) => return Ok(internal("[workbench/git-hosts] host read failed", e)),
    };
    let sb = state.secretbox().await.unwrap_or_default();
    let host = talaria_git_hosts::connect(&state.pg, &sb, &row).await;
    let status = host.status().await;
    let (repos, repos_error) = if status.configured && status.error.is_none() {
        match host.reachable_repos().await {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e)),
        }
    } else {
        (None, None)
    };
    let ok = status.configured && status.error.is_none() && repos_error.is_none();
    Ok(Json(json!({
        "ok": ok,
        "status": status,
        "repoCount": repos.as_ref().map(Vec::len),
        "repos": repos.map(|r| r.into_iter().take(SAMPLE).collect::<Vec<_>>()),
        "reposError": repos_error,
    }))
    .into_response())
}
