// /api/workbench/repos/{agentId}. Per-agent workbench repo grants —
// explicit, like MCP assignment, and per HOST: the same owner/name on two
// git hosts is two repositories. GET → every registered host's reachable
// pool + this agent's grants (and each grant's branch rules and live branch
// list); PUT → replace the grant set (validated against the pool).
// agents.manage.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::{self as hosts, GitHost, RepoRef, RepoRule};
use talaria_session::{require_agent_manager, require_agent_reader};
use talaria_state::AppState;

async fn agent_exists(pg: &sqlx::PgPool, id: &str) -> bool {
    sqlx::query_scalar::<_, i32>("select 1 from agent_defs where id = $1::uuid")
        .bind(id)
        .fetch_optional(pg)
        .await
        .map(|r| r.is_some())
        .unwrap_or(false)
}

/// The map key a grant's branches ride under: `host:owner/name`.
fn key_of(host: &str, repo: &str) -> String {
    format!("{host}:{repo}")
}

/// One `{host, repo}` element of the body.
fn repo_ref(el: &Value) -> Result<RepoRef, String> {
    let Some(entry) = el.as_object() else {
        return Err("each repo is an object: {host, repo}".into());
    };
    Ok(RepoRef {
        host_id: string_member(entry, "host", 1, 40)?,
        repo: string_member(entry, "repo", 3, 200)?,
    })
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Result<Response, Response> {
    require_agent_reader(&state, &headers, &agent_id).await?;
    if !agent_exists(&state.pg, &agent_id).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent"));
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let mut available: Vec<Value> = Vec::new();
    let mut host_list: Vec<Value> = Vec::new();
    for (row, reached) in hosts::reachable_pool(&state.pg, &sb).await {
        let error = reached.as_ref().err().cloned();
        for repo in reached.unwrap_or_default() {
            available.push(json!({ "host": row.id, "repo": repo }));
        }
        host_list.push(json!({
            "id": row.id,
            "name": row.name,
            "kind": row.kind.as_str(),
            "error": error,
        }));
    }
    let granted = hosts::granted_repos(&state.pg, &agent_id).await;
    // The rules live beside the grants, and each granted repo carries its
    // live branch list for the pickers — the admin configures against what
    // actually exists, not a free-text guess.
    let rules = hosts::repo_rules(&state.pg, &agent_id).await;
    let mut branches = Map::new();
    for rule in &rules {
        let list = match hosts::host(&state.pg, &sb, &rule.host_id).await {
            Ok(host) => host.list_branches(&rule.repo).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        branches.insert(key_of(&rule.host_id, &rule.repo), json!(list));
    }
    Ok(Json(json!({
        "hosts": host_list,
        "available": available,
        "granted": granted,
        "rules": rules,
        "branches": branches,
    }))
    .into_response())
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    require_agent_manager(&state, &headers, &agent_id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Required array of {host, repo} (min 0 — an empty PUT clears the
    // grants), ≤100 items. Validation runs before the agent check, the agent
    // check before the pool filter.
    let Some(arr) = obj.get("repos").and_then(Value::as_array) else {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "repos must be an array of {host, repo}",
        ));
    };
    if arr.len() > 100 {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            "at most 100 repos per agent",
        ));
    }
    let mut asked: Vec<RepoRef> = Vec::with_capacity(arr.len());
    for el in arr {
        match repo_ref(el) {
            Ok(r) => asked.push(r),
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        }
    }
    if !agent_exists(&state.pg, &agent_id).await {
        return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent"));
    }
    let sb = state.secretbox().await.unwrap_or_default();
    let pool: Vec<RepoRef> = hosts::reachable_pool(&state.pg, &sb)
        .await
        .into_iter()
        .flat_map(|(row, reached)| {
            reached
                .unwrap_or_default()
                .into_iter()
                .map(move |repo| RepoRef {
                    host_id: row.id.clone(),
                    repo,
                })
        })
        .collect();
    // Silently dropped, not rejected: a repo that fell out of its host's
    // reach since the UI loaded isn't a user error.
    let repos: Vec<RepoRef> = asked.into_iter().filter(|r| pool.contains(r)).collect();
    if let Err(e) = hosts::set_granted_repos(&state.pg, &agent_id, &repos).await {
        return Ok(internal("[workbench/repos] grant write failed", e));
    }
    // Rules ride the same PUT, optional: an array of {host, repo,
    // baseBranch?, pushMode?, branchPrefix?}. Only rules for STILL-GRANTED
    // repos are honored — a rule for a repo the same request revoked is a UI
    // race, and the quiet drop is the honest resolution.
    if let Some(v) = obj.get("rules") {
        let Some(arr) = v.as_array() else {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "rules must be an array",
            ));
        };
        for el in arr {
            let Some(entry) = el.as_object() else {
                return Ok(house_error(
                    StatusCode::BAD_REQUEST,
                    "rules entries must be objects",
                ));
            };
            let target = match repo_ref(el) {
                Ok(r) => r,
                Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
            };
            if !repos.contains(&target) {
                continue;
            }
            let rule = RepoRule {
                host_id: target.host_id,
                repo: target.repo,
                base_branch: talaria_body::optional_max_string_member(entry, "baseBranch", 200)
                    .ok()
                    .flatten()
                    .filter(|b| !b.is_empty()),
                push_mode: {
                    match talaria_body::enum_member(entry, "pushMode", &["branches_only", "free"]) {
                        Ok(m) => m.to_string(),
                        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
                    }
                },
                // Trimmed of '/' at both ends: a prefix typed "agent/" would
                // otherwise compose into `agent//<ref>` — a branch git hosts'
                // ref validation refuses (the mint normalizes too, for rows
                // written before this guard).
                branch_prefix: talaria_body::optional_max_string_member(entry, "branchPrefix", 100)
                    .ok()
                    .flatten()
                    .map(|p| p.trim().trim_matches('/').to_string())
                    .filter(|p| !p.is_empty()),
            };
            if let Err(e) = hosts::set_repo_rule(&state.pg, &agent_id, &rule).await {
                return Ok(internal("[workbench/repos] rule write failed", e));
            }
        }
    }
    Ok(Json(json!({ "granted": repos })).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_names_its_host_and_its_repo() {
        let r = repo_ref(&json!({ "host": "forge", "repo": "acme/notes" })).unwrap();
        assert_eq!(
            (r.host_id.as_str(), r.repo.as_str()),
            ("forge", "acme/notes")
        );
        assert!(
            repo_ref(&json!("acme/notes")).is_err(),
            "a bare repo names no host"
        );
        assert!(repo_ref(&json!({ "repo": "acme/notes" })).is_err());
        assert_eq!(key_of("forge", "acme/notes"), "forge:acme/notes");
    }
}
