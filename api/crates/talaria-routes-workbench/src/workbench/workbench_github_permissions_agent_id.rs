// /api/workbench/github-permissions/{agentId}. The GitHub permissions one
// agent holds on each git host above the base its sandbox token carries
// (talaria-git-hosts' permissions: `agent_github_permissions`). Per agent,
// per host: what an admin gives here applies to every repo the agent is
// granted on that host, and rides on each repo's token where git spends it
// (`workflows`); a permission a Talaria tool spends never does.
//
// GET → each host the agent holds a repo or a permission on: what it holds,
// what the host's App asks for (from the holdings record, never GitHub), and
// each granted repo with the installation that mints its token, what that
// installation accepted, the shortfall, and its default branch's protection;
// plus the base and the catalog of what may be granted. The agent's managers
// and fleet readers read it, and `canEdit` says whether this caller writes.
//
// PUT {host, from, to, acknowledge?} → ADMIN ONLY: an agent's managers see
// these read-only (docs/PERMISSIONS.md, "Agent managers"). A compare-and-set:
// `from` is what the editor read, and a write that finds anything else
// answers 409 with what is there now. `to` is checked key by key against the
// catalog's allowlist, and what it adds against what the host's App asks for
// (unknown is allowed, and said). Adding `workflows` reads each granted
// repo's default-branch protection, and a repo not known to need a pull
// request needs `acknowledge: true`, which the audit row records. A host
// whose token cannot be narrowed (a PAT, Forgejo) takes nothing: its own
// token decides. Taking anything back revokes the agent's tokens on the host
// in every api process.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_body::{optional_boolean_member, parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::holdings::{InstallationRow, installation_rows, tracked_installations};
use talaria_git_hosts::permissions::{
    self, AgentHostPermissions, Cas, PermissionsChange, default_branch_protections,
    installation_for_repo, set_agent_host_permissions_cas,
};
use talaria_git_hosts::{self as hosts, HostKind, HostRow, Narrowing, PermissionSet};
use talaria_github::{
    BranchProtection, GRANTABLE, ProtectionState, SANDBOX_TOKEN_PERMISSIONS, validate_extras,
};
use talaria_realtime_watch::RealtimeDeps;
use talaria_session::{actor_of, require_admin, require_agent_reader};
use talaria_state::AppState;

/// The agent's display name, or None when there is no such agent.
async fn agent_name(pg: &sqlx::PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("select display_name from agent_defs where id = $1::uuid")
        .bind(id)
        .fetch_optional(pg)
        .await
}

/// What the host's App asks for, from the record: the same registration
/// answers for every installation, so the first selected one read says it.
/// None when no read has said.
fn app_permissions(selected: &[String], rows: &[InstallationRow]) -> Option<PermissionSet> {
    selected.iter().find_map(|id| {
        rows.iter()
            .find(|r| &r.installation_id == id)
            .and_then(|r| r.app_permissions.clone())
    })
}

/// The 409 a write measured from a stale read gets, with what is there now.
fn changed_since(current: &PermissionSet) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "this agent's GitHub permissions on that host changed since you read them — read them again",
            "current": current.to_json(),
        })),
    )
        .into_response()
}

/// One `{key: level}` member of the body, as a permission set.
fn permission_member(obj: &Map<String, Value>, key: &str) -> Result<PermissionSet, String> {
    match obj.get(key) {
        Some(v @ Value::Object(_)) => {
            PermissionSet::from_json(v).map_err(|e| format!("{key}: {e}"))
        }
        _ => Err(format!(
            "{key} must be an object of permission → level, like {{\"workflows\": \"write\"}}"
        )),
    }
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
) -> Result<Response, Response> {
    let user = require_agent_reader(&state, &headers, &agent_id).await?;
    if let Some(gate) = talaria_params::uuid_gate_404(&agent_id) {
        return Ok(gate);
    }
    match agent_name(&state.pg, &agent_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent")),
        Err(e) => {
            return Ok(internal(
                "[workbench/github-permissions] agent read failed",
                e,
            ));
        }
    }
    let held = match permissions::agent_permissions(&state.pg, &agent_id).await {
        Ok(h) => h,
        Err(e) => return Ok(internal("[workbench/github-permissions] read failed", e)),
    };
    let rows = match hosts::host_rows(&state.pg).await {
        Ok(r) => r,
        Err(e) => {
            return Ok(internal(
                "[workbench/github-permissions] host read failed",
                e,
            ));
        }
    };
    let grants = hosts::granted_repos(&state.pg, &agent_id).await;
    let sb = state.secretbox().await.unwrap_or_default();
    let mut out: Vec<Value> = Vec::new();
    for row in &rows {
        let repos: Vec<String> = grants
            .iter()
            .filter(|g| g.host_id == row.id)
            .map(|g| g.repo.clone())
            .collect();
        let mine = held.iter().find(|h| h.host_id == row.id);
        if repos.is_empty() && mine.is_none() {
            continue;
        }
        let narrowing = hosts::narrowing_of(row);
        let protection = if row.kind == HostKind::Github && narrowing == Narrowing::PerRepo {
            default_branch_protections(&hosts::connect(&state.pg, &sb, row).await, &repos).await
        } else {
            Vec::new()
        };
        out.push(host_wire(&state.pg, row, narrowing, &repos, mine, &protection).await);
    }
    Ok(Json(json!({
        "agentId": agent_id,
        "canEdit": user.role == "admin",
        "base": PermissionSet::of(SANDBOX_TOKEN_PERMISSIONS).to_json(),
        "grantable": GRANTABLE.iter().map(|g| g.wire()).collect::<Vec<_>>(),
        "hosts": out,
    }))
    .into_response())
}

/// One host's block of the GET: `protection` is each granted repo's
/// default-branch read, empty on a host that has none to give.
async fn host_wire(
    pg: &sqlx::PgPool,
    row: &HostRow,
    narrowing: Narrowing,
    repos: &[String],
    mine: Option<&AgentHostPermissions>,
    protection: &[BranchProtection],
) -> Value {
    let extras = mine.map(|m| m.permissions.clone()).unwrap_or_default();
    let selected = tracked_installations(row);
    let installations = installation_rows(pg, &row.id).await.unwrap_or_else(|e| {
        tracing::warn!("[workbench/github-permissions] {e}");
        Vec::new()
    });
    let wants = PermissionSet::of(SANDBOX_TOKEN_PERMISSIONS).union(&extras);
    let repo_wire: Vec<Value> = repos
        .iter()
        .map(|repo| {
            let on = installation_for_repo(&selected, &installations, repo);
            let held = on.and_then(|r| r.held.as_ref());
            json!({
                "repo": repo,
                "installation": on.map(|r| r.installation_id.clone()),
                "held": held.map(PermissionSet::to_json),
                "shortfall": held.map(|h| wants.missing_from(h).to_json()),
                "protection": protection.iter().find(|p| &p.repo == repo).map(BranchProtection::wire),
            })
        })
        .collect();
    json!({
        "host": row.id,
        "name": row.name,
        "kind": row.kind.as_str(),
        "narrowing": narrowing.wire(),
        "hostWide": match narrowing {
            Narrowing::HostWide(why) => Some(why),
            Narrowing::PerRepo => None,
        },
        "permissions": extras.to_json(),
        "updatedBy": mine.and_then(|m| m.updated_by.clone()),
        "updatedAt": mine.map(|m| epoch_ms_to_iso(m.updated_at_ms)),
        "appPermissions": app_permissions(&selected, &installations).map(|a| a.to_json()),
        "repos": repo_wire,
    })
}

pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(agent_id): Path<String>,
    body: Bytes,
) -> Result<Response, Response> {
    // Admins only, whoever manages the agent: what a GitHub token may do is
    // the git-host plane's, which is admin-only.
    let user = require_admin(&state, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let host_id = match string_member(obj, "host", 1, 40) {
        Ok(h) => h,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let from = match permission_member(obj, "from") {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let to = match permission_member(obj, "to") {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let acknowledge = match optional_boolean_member(obj, "acknowledge") {
        Ok(a) => a.unwrap_or(false),
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    if let Some(gate) = talaria_params::uuid_gate_404(&agent_id) {
        return Ok(gate);
    }
    let agent_label = match agent_name(&state.pg, &agent_id).await {
        Ok(Some(name)) => name,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent")),
        Err(e) => {
            return Ok(internal(
                "[workbench/github-permissions] agent read failed",
                e,
            ));
        }
    };
    let row = match hosts::get_host(&state.pg, &host_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "no such git host")),
        Err(e) => {
            return Ok(internal(
                "[workbench/github-permissions] host read failed",
                e,
            ));
        }
    };
    // A stale read is told so before anything is checked against it: the
    // gates below are measured from `from`, and an editor that read before
    // someone else wrote must read again. The write itself compares again,
    // so a change landing after this read is refused the same way.
    match permissions::host_permissions(&state.pg, &agent_id, &row.id).await {
        Ok(current) if current != from => return Ok(changed_since(&current)),
        Ok(_) => {}
        Err(e) => return Ok(internal("[workbench/github-permissions] read failed", e)),
    }
    // A host whose token cannot be narrowed takes nothing: no grant here
    // would be enforced, so none is claimed.
    if !to.is_empty()
        && let Narrowing::HostWide(why) = hosts::narrowing_of(&row)
    {
        return Ok(house_error(StatusCode::BAD_REQUEST, why));
    }
    // The allowlist, key by key, each at a level it is granted at.
    if let Err(why) = validate_extras(&to) {
        return Ok(house_error(StatusCode::BAD_REQUEST, &why));
    }
    let gained = to.missing_from(&from);
    let mut notes: Vec<String> = Vec::new();
    // What the App's registration asks for is the ceiling: a permission it
    // does not ask for is one no installation can accept. The record says;
    // when it cannot, the grant stands and the answer says so.
    if !gained.is_empty() {
        let selected = tracked_installations(&row);
        let installations = installation_rows(&state.pg, &row.id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("[workbench/github-permissions] {e}");
                Vec::new()
            });
        match app_permissions(&selected, &installations) {
            Some(app) => {
                let lacking = gained.missing_from(&app);
                if !lacking.is_empty() {
                    return Ok(house_error(
                        StatusCode::BAD_REQUEST,
                        &format!(
                            "the GitHub App on {} does not ask for {lacking} — the App's owner adds it to the App on GitHub, then an org owner accepts it; Talaria can give an agent only what the App asks for",
                            row.name
                        ),
                    ));
                }
            }
            None => notes.push(format!(
                "what the GitHub App on {} asks for is not known yet (Admin → Git hosts → Refresh reads it), so whether GitHub will give {gained} is not known",
                row.name
            )),
        }
    }
    // The `workflows` gate: each granted repo's default branch. Advisory —
    // Talaria does not set the repository's branch policy — but a repo not
    // known to need a pull request is granted only with the admin's
    // acknowledgement, which the audit row keeps.
    let mut protection: Vec<BranchProtection> = Vec::new();
    if gained.contains("workflows") {
        let repos: Vec<String> = hosts::granted_repos(&state.pg, &agent_id)
            .await
            .into_iter()
            .filter(|g| g.host_id == row.id)
            .map(|g| g.repo)
            .collect();
        let sb = state.secretbox().await.unwrap_or_default();
        protection =
            default_branch_protections(&hosts::connect(&state.pg, &sb, &row).await, &repos).await;
        let exposed: Vec<&BranchProtection> = protection
            .iter()
            .filter(|p| p.state != ProtectionState::Protected)
            .collect();
        if !exposed.is_empty() && !acknowledge {
            let each: Vec<String> = exposed
                .iter()
                .map(|p| format!("{} is {} ({})", p.repo, p.state.as_str(), p.why))
                .collect();
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!(
                        "workflows lets this agent's sandbox change what CI runs, and on {} a direct push may land on a default branch: {}. Send acknowledge: true to give it anyway",
                        row.name,
                        each.join("; ")
                    ),
                    "protection": protection.iter().map(BranchProtection::wire).collect::<Vec<_>>(),
                })),
            )
                .into_response());
        }
    }
    let protection_wire: Vec<Value> = protection.iter().map(BranchProtection::wire).collect();
    let note = json!({
        "acknowledged": acknowledge
            && protection.iter().any(|p| p.state != ProtectionState::Protected),
        "protection": protection_wire,
        "notes": notes,
    });
    let actor = actor_of(&user);
    let change = PermissionsChange {
        actor: &actor,
        user_id: Some(&user.id),
        agent_label: &agent_label,
        note,
    };
    match set_agent_host_permissions_cas(&state.pg, &agent_id, &row.id, &from, &to, &change).await {
        Ok(Cas::Ok) => {}
        Ok(Cas::Changed(current)) => return Ok(changed_since(&current)),
        Err(e) => return Ok(internal("[workbench/github-permissions] write failed", e)),
    }
    // Anything taken back is taken out of every token already handed out.
    let taken = from.missing_from(&to);
    if !taken.is_empty() {
        let rt = RealtimeDeps::publish_only(state.redis().await.ok());
        talaria_git_hosts::revoke::revoke_agent_tokens(
            &state.pg,
            &rt,
            Some(row.id.as_str()),
            &agent_id,
            None,
            &format!("an admin took back {taken} on {}", row.id),
        )
        .await;
    }
    Ok(Json(json!({
        "ok": true,
        "host": row.id,
        "permissions": to.to_json(),
        "protection": protection_wire,
        "notes": notes,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_git_hosts::Level;

    #[test]
    fn a_permission_member_is_an_object_of_levels() {
        let body = json!({ "from": {}, "to": { "workflows": "write" }, "bad": ["workflows"] });
        let obj = body.as_object().unwrap();
        assert!(permission_member(obj, "from").unwrap().is_empty());
        assert_eq!(
            permission_member(obj, "to").unwrap(),
            PermissionSet::of(&[("workflows", Level::Write)])
        );
        assert!(permission_member(obj, "bad").is_err());
        assert!(permission_member(obj, "missing").is_err());
        let odd = json!({ "to": { "workflows": "sometimes" } });
        assert!(permission_member(odd.as_object().unwrap(), "to").is_err());
    }

    #[test]
    fn the_apps_ask_is_read_from_the_first_selected_installation_that_says() {
        let row = |id: &str, app: Option<PermissionSet>| InstallationRow {
            installation_id: id.into(),
            app_permissions: app,
            ..InstallationRow::default()
        };
        let wf = PermissionSet::of(&[("workflows", Level::Write)]);
        let rows = vec![row("91", None), row("92", Some(wf.clone()))];
        assert_eq!(
            app_permissions(&["91".into(), "92".into()], &rows),
            Some(wf)
        );
        assert_eq!(app_permissions(&["91".into()], &rows), None);
        assert_eq!(app_permissions(&[], &rows), None);
    }

    // ── Live-DB: the route end to end ────────────────────────────────────────
    //
    // #[ignore]d (house rule: no service-dependent tests in the default run;
    // ci.yml's api job sweeps ignored lib tests in its live-DB steps, against
    // its scratch database and Redis). GitHub is a loopback stub the host
    // row's API origin points at.
    //
    //   DATABASE_URL=postgres://… REDIS_URL=… TALARIA_SECRET_KEY=… \
    //     cargo test -p talaria-routes-workbench github_permissions -- --ignored

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
        async fn only_an_admin_gives_workflows_and_a_narrowing_takes_the_token_back_everywhere() {
            let state = state().await;
            let t = tag();
            let (admin, as_admin) = person(&state, &t, "admin", "admin").await;
            let (manager, as_manager) = person(&state, &t, "manager", "member").await;
            let a = agent(&state, &t, &manager).await;
            let (base, log) = github("95001").await;
            let h = github_host(&state, &t, &base, "95001").await;
            talaria_git_hosts::add_grant(&state.pg, &a, &h, "acme/widgets")
                .await
                .unwrap();
            let give = json!({ "host": h, "from": {}, "to": { "workflows": "write" } });

            // The agent's manager may not: it is an admin's.
            let (status, _) = put_as(&state, &as_manager, &a, give.clone()).await;
            assert_eq!(status, StatusCode::FORBIDDEN);

            // workflows on a repo whose default branch takes a direct push:
            // not without the admin's acknowledgement, and the answer says
            // which repo and why.
            let (status, body) = put_as(&state, &as_admin, &a, give.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(body["protection"][0]["repo"], "acme/widgets");
            assert_eq!(body["protection"][0]["state"], "unprotected");
            assert!(
                body["error"].as_str().unwrap().contains("acknowledge"),
                "{body}"
            );
            let mut acknowledged = give.clone();
            acknowledged["acknowledge"] = json!(true);
            let (status, body) = put_as(&state, &as_admin, &a, acknowledged).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["permissions"], json!({ "workflows": "write" }));
            let row: Value = sqlx::query_scalar(
                "select after from audit_log where action = 'workbench.github_permissions' \
                 and target_id = $1",
            )
            .bind(&a)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            assert_eq!(row["acknowledged"], true, "{row}");

            // A second admin who read before that write: 409, with what is
            // there now.
            let (status, body) = put_as(&state, &as_admin, &a, give.clone()).await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert_eq!(body["current"], json!({ "workflows": "write" }));

            // The allowlist: a key never given, and a level the key is not
            // given at.
            for to in [
                json!({ "administration": "write" }),
                json!({ "workflows": "read" }),
            ] {
                let body = json!({ "host": h, "from": { "workflows": "write" }, "to": to });
                let (status, _) = put_as(&state, &as_admin, &a, body).await;
                assert_eq!(status, StatusCode::BAD_REQUEST);
            }

            // The manager reads it, and may not write.
            let (status, body) =
                answer(get(State(state.clone()), as_manager.clone(), Path(a.clone())).await).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["canEdit"], false);
            assert_eq!(body["grantable"][0]["key"], "workflows");
            let block = &body["hosts"][0];
            assert_eq!(block["host"], h.as_str());
            assert_eq!(block["narrowing"], "perRepo");
            assert_eq!(block["permissions"], json!({ "workflows": "write" }));
            assert_eq!(block["repos"][0]["installation"], "95001");
            assert_eq!(block["repos"][0]["shortfall"], json!({}));
            assert_eq!(block["repos"][0]["protection"]["state"], "unprotected");

            // The agent's git gets a token carrying workflows.
            let sb = state.secretbox().await.unwrap();
            let row = talaria_git_hosts::get_host(&state.pg, &h)
                .await
                .unwrap()
                .unwrap();
            let token = talaria_git_hosts::grant_login(&state.pg, &sb, &row, "acme/widgets", &a)
                .await
                .unwrap()
                .expect("a credential");
            assert_eq!(
                token.carried.as_ref().and_then(|c| c.get("workflows")),
                Some(talaria_git_hosts::Level::Write)
            );

            // Taking it back revokes that token here and tells every process.
            let subscribe = RealtimeDeps::streams_only(&state.cfg.redis_url).subscribe;
            let mut heard = subscribe(talaria_git_hosts::revoke::TOKEN_REVOKE_TOPIC.into()).await;
            let take = json!({ "host": h, "from": { "workflows": "write" }, "to": {} });
            let (status, body) = put_as(&state, &as_admin, &a, take).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert!(
                calls(&log, "DELETE", "/installation/token")
                    .contains(&format!("Bearer {}", token.login.password)),
                "the token was revoked on GitHub"
            );
            let message = tokio::time::timeout(std::time::Duration::from_secs(10), heard.recv())
                .await
                .expect("the broadcast arrives")
                .expect("a message");
            let message = talaria_git_hosts::revoke::read_broadcast(&message).expect("readable");
            assert_eq!(message.agent, a);
            assert_eq!(message.host.as_deref(), Some(h.as_str()));

            cleanup(&state, &[&a], &[&admin, &manager], &[&h]).await;
        }
    }
}
