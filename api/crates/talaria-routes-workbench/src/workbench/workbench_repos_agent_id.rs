// /api/workbench/repos/{agentId}. Per-agent workbench repo grants —
// explicit, like MCP assignment, and per HOST: the same owner/name on two
// git hosts is two repositories. GET → every registered host's reachable
// pool + this agent's grants (and each grant's branch rules and live branch
// list); PUT → replace the grant set (validated against the pool; a host
// whose reach could not be read keeps the grants it has). The agent's
// managers. Every grant added or removed is audited
// (`workbench.repo_grant`), and a removed grant's tokens are revoked in
// every api process.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use talaria_audit::{AuditEntry, log_audit};
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::permissions::{self, AgentHostPermissions};
use talaria_git_hosts::{self as hosts, GitHost, GrantChange, RepoRef, RepoRule};
use talaria_realtime_watch::RealtimeDeps;
use talaria_session::{actor_of, require_agent_manager, require_agent_reader};
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
    let user = require_agent_manager(&state, &headers, &agent_id).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    // Required array of {host, repo} (min 0 — an empty PUT clears the
    // grants), ≤100 items. Validation runs before the agent check, the agent
    // check before the pool filter, and every check before any write.
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
    // Rules ride the same PUT, optional: an array of {host, repo,
    // baseBranch?, pushMode?, branchPrefix?}, read in full before anything
    // is written, so a bad entry never leaves the grants half saved.
    let rules = match obj.get("rules") {
        None => Vec::new(),
        Some(v) => match rules_of(v) {
            Ok(r) => r,
            Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
        },
    };
    let Some(agent_label) = agent_name(&state.pg, &agent_id).await else {
        return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent"));
    };
    let sb = state.secretbox().await.unwrap_or_default();
    let current = hosts::granted_repos(&state.pg, &agent_id).await;
    let mut pool: Vec<RepoRef> = Vec::new();
    let mut unread: Vec<String> = Vec::new();
    for (row, reached) in hosts::reachable_pool(&state.pg, &sb).await {
        match reached {
            Ok(repos) => pool.extend(repos.into_iter().map(|repo| RepoRef {
                host_id: row.id.clone(),
                repo,
            })),
            Err(_) => unread.push(row.id),
        }
    }
    // Silently dropped, not rejected: a repo that fell out of its host's
    // reach since the UI loaded isn't a user error. A host whose reach could
    // not be read is not a host that reaches nothing: the grants the agent
    // already holds there are kept when the body keeps them (a new one there
    // cannot be checked against the pool, so it waits for the host).
    let repos: Vec<RepoRef> = asked
        .into_iter()
        .filter(|r| pool.contains(r) || (unread.contains(&r.host_id) && current.contains(r)))
        .collect();
    // The set an admin gave the agent on each host (talaria-git-hosts'
    // permissions), for the audit row: a grant added there carries it.
    let held: Vec<AgentHostPermissions> =
        match permissions::agent_permissions(&state.pg, &agent_id).await {
            Ok(h) => h,
            Err(e) => return Ok(internal("[workbench/repos] permissions read failed", e)),
        };
    let change = match hosts::set_granted_repos(&state.pg, &agent_id, &repos).await {
        Ok(c) => c,
        Err(e) => return Ok(internal("[workbench/repos] grant write failed", e)),
    };
    // The grant write has landed: it is audited, and a removed grant's tokens
    // taken back, before anything else can fail and answer early.
    if !change.is_empty() {
        record_grant_change(&state, &user, &agent_id, &agent_label, &change, &held).await;
    }
    // Only rules for STILL-GRANTED repos are honored — a rule for a repo the
    // same request revoked is a UI race, and the quiet drop is the honest
    // resolution.
    for rule in &rules {
        let target = RepoRef {
            host_id: rule.host_id.clone(),
            repo: rule.repo.clone(),
        };
        if !repos.contains(&target) {
            continue;
        }
        if let Err(e) = hosts::set_repo_rule(&state.pg, &agent_id, rule).await {
            return Ok(internal("[workbench/repos] rule write failed", e));
        }
    }
    Ok(Json(json!({ "granted": repos })).into_response())
}

/// The agent's display name, or None when there is no such agent.
async fn agent_name(pg: &sqlx::PgPool, id: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>("select display_name from agent_defs where id = $1::uuid")
        .bind(id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten()
}

/// The `rules` member: each `{host, repo, baseBranch?, pushMode, branchPrefix?}`.
fn rules_of(v: &Value) -> Result<Vec<RepoRule>, String> {
    let Some(arr) = v.as_array() else {
        return Err("rules must be an array".into());
    };
    let mut out = Vec::with_capacity(arr.len());
    for el in arr {
        let Some(entry) = el.as_object() else {
            return Err("rules entries must be objects".into());
        };
        let target = repo_ref(el)?;
        out.push(RepoRule {
            host_id: target.host_id,
            repo: target.repo,
            base_branch: talaria_body::optional_max_string_member(entry, "baseBranch", 200)
                .ok()
                .flatten()
                .filter(|b| !b.is_empty()),
            push_mode: talaria_body::enum_member(entry, "pushMode", &["branches_only", "free"])?
                .to_string(),
            // Trimmed of '/' at both ends: a prefix typed "agent/" would
            // otherwise compose into `agent//<ref>` — a branch git hosts' ref
            // validation refuses (the mint normalizes too, for rows written
            // before this guard).
            branch_prefix: talaria_body::optional_max_string_member(entry, "branchPrefix", 100)
                .ok()
                .flatten()
                .map(|p| p.trim().trim_matches('/').to_string())
                .filter(|p| !p.is_empty()),
        });
    }
    Ok(out)
}

/// What a grant write changed, recorded and acted on: one
/// `workbench.repo_grant` audit row with every grant added and removed (each
/// with the GitHub permissions set an admin gave the agent on its host, when
/// one did: an added grant's token carries it), and the agent's tokens for
/// each removed repo revoked in every api process.
async fn record_grant_change(
    state: &AppState,
    user: &talaria_session::SessionUser,
    agent_id: &str,
    agent_label: &str,
    change: &GrantChange,
    held: &[AgentHostPermissions],
) {
    let on_host = |host: &str| {
        held.iter()
            .find(|h| h.host_id == host)
            .map(|h| h.permissions.to_json())
    };
    let entry = |g: &RepoRef| {
        let mut entry = json!({ "host": g.host_id, "repo": g.repo });
        if let Some(permissions) = on_host(&g.host_id) {
            entry["hostPermissions"] = permissions;
        }
        entry
    };
    let added: Vec<Value> = change.added.iter().map(entry).collect();
    let removed: Vec<Value> = change.removed.iter().map(entry).collect();
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor_of(user),
            action: "workbench.repo_grant",
            target_type: "agent",
            target_id: Some(agent_id),
            target_label: Some(agent_label),
            before: None,
            after: Some(json!({ "added": added, "removed": removed })),
        },
    )
    .await;
    if change.removed.is_empty() {
        return;
    }
    let rt = RealtimeDeps::publish_only(state.redis().await.ok());
    for g in &change.removed {
        talaria_git_hosts::revoke::revoke_agent_tokens(
            &state.pg,
            &rt,
            Some(g.host_id.as_str()),
            agent_id,
            Some(g.repo.as_str()),
            &format!("the grant of {} on {} was removed", g.repo, g.host_id),
        )
        .await;
    }
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

    #[test]
    fn rules_are_read_whole_before_anything_is_written() {
        let rules = rules_of(&json!([
            { "host": "github", "repo": "acme/site", "pushMode": "branches_only", "branchPrefix": "/agent/" },
            { "host": "github", "repo": "acme/app", "pushMode": "free", "baseBranch": "" },
        ]))
        .unwrap();
        assert_eq!(rules[0].branch_prefix.as_deref(), Some("agent"));
        assert_eq!(rules[1].push_mode, "free");
        assert_eq!(rules[1].base_branch, None, "a blank base is the default");
        assert!(rules_of(&json!({})).is_err());
        assert!(rules_of(&json!(["acme/site"])).is_err());
        assert!(
            rules_of(&json!([{ "host": "github", "repo": "acme/site", "pushMode": "yolo" }]))
                .is_err()
        );
    }

    // ── Live-DB: the grants editor end to end ────────────────────────────────
    //
    // #[ignore]d; ci.yml's api job runs it in its live-DB steps.
    //
    //   DATABASE_URL=postgres://… REDIS_URL=… TALARIA_SECRET_KEY=… \
    //     cargo test -p talaria-routes-workbench repos_agent_id -- --ignored

    mod live {
        use super::super::*;
        use crate::workbench::live_support::{
            agent, answer, calls, cleanup, github, github_host, person, state, tag,
        };

        async fn put_as(
            state: &AppState,
            headers: &HeaderMap,
            agent: &str,
            body: Value,
        ) -> (StatusCode, Value) {
            answer(
                put(
                    State(state.clone()),
                    headers.clone(),
                    Path(agent.to_string()),
                    Bytes::from(body.to_string()),
                )
                .await,
            )
            .await
        }

        #[tokio::test]
        #[ignore = "needs a live database and Redis (DATABASE_URL, REDIS_URL)"]
        async fn a_host_that_cannot_be_read_keeps_its_grants_and_every_change_is_audited() {
            let state = state().await;
            let t = tag();
            let (admin, as_admin) = person(&state, &t, "admin", "admin").await;
            let (manager, as_manager) = person(&state, &t, "manager", "member").await;
            let a = agent(&state, &t, &manager).await;
            let (base, log) = github("95101").await;
            let gh = github_host(&state, &t, &base, "95101").await;
            // A Forgejo host whose token secret was never made: its reach
            // cannot be read.
            let forge = format!("wb-live-forge-{t}");
            sqlx::query(
                "insert into git_hosts (id, kind, name, base_url, auth) \
                 values ($1, 'forgejo', $1, $2, '{}'::jsonb)",
            )
            .bind(&forge)
            .bind(format!("https://{forge}.example.invalid"))
            .execute(&state.pg)
            .await
            .unwrap();
            talaria_git_hosts::add_grant(&state.pg, &a, &forge, "acme/notes")
                .await
                .unwrap();

            // The manager adds a GitHub repo and keeps the Forgejo one: the
            // unreadable host's grant stays.
            let both = json!({ "repos": [
                { "host": gh, "repo": "acme/widgets" },
                { "host": forge, "repo": "acme/notes" },
            ] });
            let (status, body) = put_as(&state, &as_manager, &a, both.clone()).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["granted"].as_array().map(Vec::len), Some(2), "{body}");
            let row: Value = sqlx::query_scalar(
                "select after from audit_log where action = 'workbench.repo_grant' \
                 and target_id = $1 order by created_at desc limit 1",
            )
            .bind(&a)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            assert_eq!(
                row,
                json!({ "added": [{ "host": gh, "repo": "acme/widgets" }], "removed": [] })
            );

            // An admin gives the agent a set on the host; its manager still
            // sets the branch rules of its repos there, as before.
            sqlx::query(
                "insert into agent_github_permissions (agent_id, host_id, permissions) \
                 values ($1::uuid, $2, '{\"contents\":\"write\",\"workflows\":\"write\"}'::jsonb)",
            )
            .bind(&a)
            .bind(&gh)
            .execute(&state.pg)
            .await
            .unwrap();
            let mut free = both.clone();
            free["rules"] = json!([{ "host": gh, "repo": "acme/widgets", "pushMode": "free" }]);
            let (status, body) = put_as(&state, &as_manager, &a, free).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            let rule = talaria_git_hosts::repo_rule(&state.pg, &a, &gh, "acme/widgets")
                .await
                .expect("granted");
            assert_eq!(rule.push_mode, "free");

            // The agent holds a token for the GitHub repo; removing the grant
            // revokes it, and the row says what the agent held on the host.
            let sb = state.secretbox().await.unwrap();
            let host_row = talaria_git_hosts::get_host(&state.pg, &gh)
                .await
                .unwrap()
                .unwrap();
            let token =
                talaria_git_hosts::grant_login(&state.pg, &sb, &host_row, "acme/widgets", &a)
                    .await
                    .unwrap()
                    .expect("a credential");
            let only_forge = json!({ "repos": [{ "host": forge, "repo": "acme/notes" }] });
            let (status, body) = put_as(&state, &as_manager, &a, only_forge).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(
                calls(&log, "DELETE", "/installation/token")
                    .contains(&format!("Bearer {}", token.login.password)),
                "the removed grant's token was revoked"
            );
            let row: Value = sqlx::query_scalar(
                "select after from audit_log where action = 'workbench.repo_grant' \
                 and target_id = $1 order by created_at desc limit 1",
            )
            .bind(&a)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            assert_eq!(
                row["removed"],
                json!([{ "host": gh, "repo": "acme/widgets", "hostPermissions": { "contents": "write", "workflows": "write" } }])
            );
            assert_eq!(
                hosts::granted_repos(&state.pg, &a).await,
                [RepoRef {
                    host_id: forge.clone(),
                    repo: "acme/notes".into()
                }],
                "the unreadable host's grant is still there"
            );

            // A host whose token cannot be narrowed takes no set.
            let (status, body) = answer(
                crate::workbench::workbench_github_permissions_agent_id::put(
                    State(state.clone()),
                    as_admin.clone(),
                    Path(a.clone()),
                    Bytes::from(
                        json!({ "host": forge, "from": null, "to": { "workflows": "write" } })
                            .to_string(),
                    ),
                )
                .await,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert!(
                body["error"].as_str().unwrap().contains("own token"),
                "{body}"
            );

            // The manager clears the rest: removed, and audited.
            let (status, _) = put_as(&state, &as_manager, &a, json!({ "repos": [] })).await;
            assert_eq!(status, StatusCode::OK);
            assert!(hosts::granted_repos(&state.pg, &a).await.is_empty());

            cleanup(&state, &[&a], &[&admin, &manager], &[&gh, &forge]).await;
        }
    }
}
