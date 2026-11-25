// /api/workbench/github-permissions/{agentId}. The GitHub permissions one
// agent's token carries on each git host (talaria-git-hosts' permissions:
// `agent_github_permissions`). Per agent, per host, the agent's WHOLE set:
// any key GitHub knows, repository and organization keys, administration
// included, applying to every repo the agent is granted on that host, and
// minted into each repo's token as far as the installation holds it. With no
// set, the agent carries the default (contents and pull requests to write,
// metadata to read).
//
// GET → each host the agent holds a repo or a set on: what it carries and
// whether that is the default, each key with whether the installation holds
// it, what the host's App asks for (from the holdings record, never GitHub),
// each installation the host uses (what it holds, what it has not accepted
// yet, its page on GitHub), and each granted repo with the installation that
// mints its token, what that installation accepted and the shortfall; plus
// the default and the catalog of every key GitHub knows. What the record
// says the installation holds is not what a token carried: each host also
// names the workspace secret that answers git there before the host's own
// login (`workspaceSecret`, by name, never opened), and each repo its
// agent's last token (`lastToken`: the newest `git.credential` or
// `git.credential_refused` row — what GitHub put on it, what it went
// without and why, what GitHub added to it unasked (`added`, null when
// nothing), or why there was none — and `current`, whether it was asked
// for the set the agent holds now). The agent's managers and fleet
// readers read it; `canEdit` says whether this caller writes, and
// `canCheck` whether it may press "Check token" (POST …/check, admins).
//
// PUT {host, from, to} → ADMIN ONLY: an agent's managers see these read-only
// (docs/PERMISSIONS.md, "Agent managers"). A compare-and-set: `from` is what
// the editor read (null: the default), and a write that finds anything else
// answers 409 with what is there now. `to` is the whole new set, each key
// one GitHub knows at a level it takes (an unknown key or level is a 400,
// and nothing else is refused), or null to put the agent back on the
// default. What the App does not ask for yet is said, not refused. A host
// whose token cannot be narrowed (a PAT, Forgejo) takes no set: its own
// token decides. Narrowing revokes the agent's tokens on the host in every
// api process.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_body::{parse, string_member};
use talaria_error::{house_error, internal, object_or_400};
use talaria_git_hosts::credential::{self, LastToken};
use talaria_git_hosts::holdings::{
    InstallationRow, installation_rows, installation_statuses, tracked_installations,
};
use talaria_git_hosts::permissions::{
    self, AgentHostPermissions, Cas, PermissionsChange, default_set, effective,
    installation_for_repo, set_agent_host_permissions_cas,
};
use talaria_git_hosts::{self as hosts, HostRow, Narrowing, PermissionSet};
use talaria_github::{CATALOG, Scope, catalog_entry, is_organization_key, validate_permissions};
use talaria_realtime_watch::RealtimeDeps;
use talaria_session::{actor_of, require_admin, require_agent_reader};
use talaria_state::AppState;

/// The agent's display name, or None when there is no such agent.
async fn agent_name(pg: &sqlx::PgPool, id: &str) -> Result<Option<String>, sqlx::Error> {
    Ok(agent_row(pg, id).await?.map(|(name, _)| name))
}

/// The agent's display name and model (what the workspace store grants by),
/// or None when there is no such agent.
pub(crate) async fn agent_row(
    pg: &sqlx::PgPool,
    id: &str,
) -> Result<Option<(String, String)>, sqlx::Error> {
    sqlx::query_as("select display_name, model from agent_defs where id = $1::uuid")
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
fn changed_since(current: Option<&PermissionSet>) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "this agent's GitHub permissions on that host changed since you read them — read them again",
            "current": current.cloned().unwrap_or_else(default_set).to_json(),
            "isDefault": current.is_none(),
        })),
    )
        .into_response()
}

/// One `{key: level}` member of the body, or null for the default. The
/// member must be there: a write says what it read and what it wants.
fn permissions_or_default_member(
    obj: &Map<String, Value>,
    key: &str,
) -> Result<Option<PermissionSet>, String> {
    match obj.get(key) {
        Some(Value::Null) => Ok(None),
        Some(v @ Value::Object(_)) => PermissionSet::from_json(v)
            .map(Some)
            .map_err(|e| format!("{key}: {e}")),
        _ => Err(format!(
            "{key} must be an object of permission → level, like {{\"contents\": \"write\"}}, or null for the default"
        )),
    }
}

/// Where a key applies, as a page reads it.
fn scope_of(key: &str) -> &'static str {
    match catalog_entry(key) {
        Some(entry) => entry.scope.as_str(),
        None if is_organization_key(key) => Scope::Organization.as_str(),
        None => Scope::Repository.as_str(),
    }
}

/// Whether the installations in `held` (each one's record, None where it was
/// never read) all hold `key` at `level`: true when every one read does,
/// false when one read does not, None when none was read.
fn held_by(held: &[Option<&PermissionSet>], key: &str, level: talaria_git_hosts::Level) -> Value {
    let known: Vec<&PermissionSet> = held.iter().flatten().copied().collect();
    if known.is_empty() {
        return Value::Null;
    }
    json!(known.iter().all(|h| h.get(key).is_some_and(|l| l >= level)))
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
    let model = match agent_row(&state.pg, &agent_id).await {
        Ok(Some((_, model))) => model,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "unknown agent")),
        Err(e) => {
            return Ok(internal(
                "[workbench/github-permissions] agent read failed",
                e,
            ));
        }
    };
    let sets = match permissions::agent_permissions(&state.pg, &agent_id).await {
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
    // What each repo's last token carried, from the trail: a read failing
    // leaves it unsaid, never the editor.
    let last = credential::last_tokens(&state.pg, std::slice::from_ref(&agent_id))
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("[workbench/github-permissions] {e}");
            Vec::new()
        });
    let mut out: Vec<Value> = Vec::new();
    for row in &rows {
        let repos: Vec<String> = grants
            .iter()
            .filter(|g| g.host_id == row.id)
            .map(|g| g.repo.clone())
            .collect();
        let mine = sets.iter().find(|h| h.host_id == row.id);
        if repos.is_empty() && mine.is_none() {
            continue;
        }
        let secret = credential::workspace_secret_at(&state.pg, &model, row, &rows)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("[workbench/github-permissions] {e}");
                None
            });
        out.push(host_wire(&state.pg, row, &repos, mine, secret, &last).await);
    }
    Ok(Json(json!({
        "agentId": agent_id,
        "canEdit": user.role == "admin",
        "canCheck": user.role == "admin",
        "default": default_set().to_json(),
        "catalog": CATALOG.iter().map(|e| e.wire()).collect::<Vec<_>>(),
        "hosts": out,
    }))
    .into_response())
}

/// The newest token row of `repo` on `host_id` among `last`, if any.
fn last_token<'a>(last: &'a [LastToken], host_id: &str, repo: &str) -> Option<&'a LastToken> {
    last.iter()
        .filter(|t| t.host_id == host_id && t.repo.eq_ignore_ascii_case(repo))
        .max_by_key(|t| t.at_ms)
}

/// One host's block of the GET. `secret` is the workspace secret that
/// answers git at the host before its own login; `last` every last token of
/// the agent.
async fn host_wire(
    pg: &sqlx::PgPool,
    row: &HostRow,
    repos: &[String],
    mine: Option<&AgentHostPermissions>,
    secret: Option<String>,
    last: &[LastToken],
) -> Value {
    let narrowing = hosts::narrowing_of(row);
    let base = json!({
        "host": row.id,
        "name": row.name,
        "kind": row.kind.as_str(),
        "narrowing": narrowing.wire(),
        "workspaceSecret": secret,
    });
    // A host whose own token decides carries no per-agent set: nothing is
    // claimed about what it carries.
    if let Narrowing::HostWide(why) = narrowing {
        let mut block = base;
        block["hostWide"] = json!(why);
        block["permissions"] = Value::Null;
        block["isDefault"] = Value::Null;
        block["repos"] = json!(
            repos
                .iter()
                .map(|r| json!({
                    "repo": r,
                    "lastToken": last_token(last, &row.id, r).map(|t| t.wire(None)),
                }))
                .collect::<Vec<_>>()
        );
        return block;
    }
    let set = effective(mine.map(|m| m.permissions.clone()));
    let selected = tracked_installations(row);
    let installations = installation_rows(pg, &row.id).await.unwrap_or_else(|e| {
        tracing::warn!("[workbench/github-permissions] {e}");
        Vec::new()
    });
    let repo_wire: Vec<Value> = repos
        .iter()
        .map(|repo| {
            let on = installation_for_repo(&selected, &installations, repo);
            let held = on.and_then(|r| r.held.as_ref());
            json!({
                "repo": repo,
                "installation": on.map(|r| r.installation_id.clone()),
                "held": held.map(PermissionSet::to_json),
                "shortfall": held.map(|h| set.missing_from(h).to_json()),
                "lastToken": last_token(last, &row.id, repo).map(|t| t.wire(Some(&set))),
            })
        })
        .collect();
    // Whether "the installation" holds each key: the installations the
    // agent's repos are minted on, or every one the host uses when it holds
    // no repo there yet.
    let minting: Vec<Option<&PermissionSet>> = if repos.is_empty() {
        installations
            .iter()
            .filter(|r| selected.contains(&r.installation_id))
            .map(|r| r.held.as_ref())
            .collect()
    } else {
        repos
            .iter()
            .filter_map(|repo| installation_for_repo(&selected, &installations, repo))
            .map(|r| r.held.as_ref())
            .collect()
    };
    let keys: Vec<Value> = set
        .iter()
        .map(|(key, level)| {
            json!({
                "key": key,
                "level": level.as_str(),
                "scope": scope_of(key),
                "held": held_by(&minting, key, level),
            })
        })
        .collect();
    let mut block = base;
    block["hostWide"] = Value::Null;
    block["permissions"] = set.to_json();
    block["isDefault"] = json!(mine.is_none());
    block["keys"] = json!(keys);
    block["updatedBy"] = json!(mine.and_then(|m| m.updated_by.clone()));
    block["updatedAt"] = json!(mine.map(|m| epoch_ms_to_iso(m.updated_at_ms)));
    block["appPermissions"] =
        json!(app_permissions(&selected, &installations).map(|a| a.to_json()));
    // Each installation the host uses, as the record has it: what it holds,
    // what the App asks for that it has not accepted yet, and its page on
    // GitHub, where an owner accepts it. The editor says why a key is not
    // held from these.
    block["installations"] = json!(installation_statuses(&selected, &installations));
    block["repos"] = json!(repo_wire);
    block
}

/// What an admin is told beside a write, none of it a refusal: what the
/// App's registration does not ask for yet (no token carries it until it
/// does and the account accepts), and that organization keys ride on a
/// repository token only where GitHub takes them there.
fn notes_for(row: &HostRow, to: &PermissionSet, app: Option<&PermissionSet>) -> Vec<String> {
    let mut notes = Vec::new();
    match app {
        Some(app) => {
            let unasked = to.missing_from(app);
            if !unasked.is_empty() {
                notes.push(format!(
                    "the GitHub App on {} does not ask for {unasked} — no token carries it until the App's owner adds it to the App on GitHub and an owner of the account accepts the change",
                    row.name
                ));
            }
        }
        None => notes.push(format!(
            "what the GitHub App on {} asks for is not known yet (Admin → Git hosts → Refresh reads it), so whether its tokens can carry all of this is not known",
            row.name
        )),
    }
    let org = to.without(|k| !is_organization_key(k));
    if !org.is_empty() {
        notes.push(format!(
            "{org} are organization permissions: they ride on the agent's token for each repository where GitHub takes them on a token for one repository, and where it does not, the token carries the rest and the credential's audit row says which GitHub refused"
        ));
    }
    notes
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
    // What the editor read: the agent's set, or null when it is on the
    // default.
    let from = match permissions_or_default_member(obj, "from") {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The whole new set, or null to put the agent back on the default.
    let to = match permissions_or_default_member(obj, "to") {
        Ok(p) => p,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    // The one check a set passes: every key one GitHub knows, at a level it
    // takes. An empty set would mint nothing at all, which is what removing
    // the agent's repo grants on the host says.
    if let Some(to) = &to {
        if to.is_empty() {
            return Ok(house_error(
                StatusCode::BAD_REQUEST,
                "an empty set gives this agent no token at all — remove its repo grants on the host instead, or send null for the default",
            ));
        }
        if let Err(why) = validate_permissions(to) {
            return Ok(house_error(StatusCode::BAD_REQUEST, &why));
        }
    }
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
    // A host whose token cannot be narrowed takes no set: none would be
    // enforced, so none is claimed.
    if let Narrowing::HostWide(why) = hosts::narrowing_of(&row) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "no per-agent GitHub permissions apply on {}: {why}",
                row.name
            ),
        ));
    }
    let from = effective(from);
    let to_effective = effective(to.clone());
    let selected = tracked_installations(&row);
    let installations = installation_rows(&state.pg, &row.id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("[workbench/github-permissions] {e}");
            Vec::new()
        });
    let notes = notes_for(
        &row,
        &to_effective,
        app_permissions(&selected, &installations).as_ref(),
    );
    let actor = actor_of(&user);
    let change = PermissionsChange {
        actor: &actor,
        user_id: Some(&user.id),
        agent_label: &agent_label,
        note: json!({ "notes": notes }),
    };
    match set_agent_host_permissions_cas(&state.pg, &agent_id, &row.id, &from, to.as_ref(), &change)
        .await
    {
        Ok(Cas::Ok) => {}
        Ok(Cas::Changed(current)) => return Ok(changed_since(current.as_ref())),
        Err(e) => return Ok(internal("[workbench/github-permissions] write failed", e)),
    }
    // Anything taken back is taken out of every token already handed out. A
    // widening needs nothing: the next ask is a new set, and a new mint.
    let taken = from.missing_from(&to_effective);
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
        "permissions": to_effective.to_json(),
        "isDefault": to.is_none(),
        "notes": notes,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_git_hosts::Level;

    #[test]
    fn a_permissions_member_is_an_object_of_levels_or_null_for_the_default() {
        let body = json!({
            "from": null,
            "to": { "administration": "write" },
            "bad": ["workflows"],
        });
        let obj = body.as_object().unwrap();
        assert_eq!(permissions_or_default_member(obj, "from").unwrap(), None);
        assert_eq!(
            permissions_or_default_member(obj, "to").unwrap(),
            Some(PermissionSet::of(&[("administration", Level::Write)]))
        );
        assert!(permissions_or_default_member(obj, "bad").is_err());
        assert!(
            permissions_or_default_member(obj, "missing").is_err(),
            "a write names what it read"
        );
        let odd = json!({ "to": { "workflows": "sometimes" } });
        assert!(permissions_or_default_member(odd.as_object().unwrap(), "to").is_err());
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

    #[test]
    fn a_key_is_held_when_every_installation_read_holds_it() {
        let a = PermissionSet::of(&[("secrets", Level::Write)]);
        let b = PermissionSet::of(&[("secrets", Level::Read)]);
        assert_eq!(held_by(&[Some(&a)], "secrets", Level::Write), json!(true));
        assert_eq!(
            held_by(&[Some(&a), Some(&b)], "secrets", Level::Write),
            json!(false)
        );
        assert_eq!(
            held_by(&[Some(&a), None], "secrets", Level::Write),
            json!(true)
        );
        assert_eq!(held_by(&[None], "secrets", Level::Read), Value::Null);
        assert_eq!(held_by(&[], "secrets", Level::Read), Value::Null);
        assert_eq!(scope_of("organization_secrets"), "organization");
        assert_eq!(scope_of("members"), "organization");
        assert_eq!(scope_of("administration"), "repository");
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
            agent, answer, bodies, calls, cleanup, github, github_host, person, state, tag,
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
        async fn an_admin_sets_the_whole_set_and_a_narrowing_takes_the_token_back_everywhere() {
            let state = state().await;
            let t = tag();
            let (admin, as_admin) = person(&state, &t, "admin", "admin").await;
            let (manager, as_manager) = person(&state, &t, "manager", "member").await;
            let a = agent(&state, &t, &manager).await;
            let b = agent(&state, &format!("{t}b"), &manager).await;
            let (base, log) = github("95001").await;
            let h = github_host(&state, &t, &base, "95001").await;
            for agent in [&a, &b] {
                talaria_git_hosts::add_grant(&state.pg, agent, &h, "acme/widgets")
                    .await
                    .unwrap();
            }
            let full = json!({
                "administration": "write",
                "organization_secrets": "read",
                "secrets": "write",
                "workflows": "write",
            });
            let give = json!({ "host": h, "from": null, "to": full });

            // The agent's manager may not: it is an admin's.
            let (status, _) = put_as(&state, &as_manager, &a, give.clone()).await;
            assert_eq!(status, StatusCode::FORBIDDEN);

            // A key GitHub does not know, or a level it does not take: 400,
            // and nothing written.
            for to in [
                json!({ "sudo": "write" }),
                json!({ "workflows": "read" }),
                json!({}),
            ] {
                let body = json!({ "host": h, "from": null, "to": to });
                let (status, body) = put_as(&state, &as_admin, &a, body).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            }

            // The full set, administration and an organization key included:
            // no gate, no acknowledgement.
            let (status, body) = put_as(&state, &as_admin, &a, give.clone()).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["permissions"], full);
            assert_eq!(body["isDefault"], false);
            let row: Value = sqlx::query_scalar(
                "select after from audit_log where action = 'workbench.github_permissions' \
                 and target_id = $1",
            )
            .bind(&a)
            .fetch_one(&state.pg)
            .await
            .unwrap();
            assert_eq!(row["permissions"], full, "{row}");
            assert_eq!(row["default"], false, "{row}");

            // A second admin who read the default before that write: 409,
            // with what is there now.
            let (status, body) = put_as(&state, &as_admin, &a, give.clone()).await;
            assert_eq!(status, StatusCode::CONFLICT, "{body}");
            assert_eq!(body["current"], full);
            assert_eq!(body["isDefault"], false);

            // The manager reads it, and may not write.
            let (status, body) =
                answer(get(State(state.clone()), as_manager.clone(), Path(a.clone())).await).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["canEdit"], false);
            assert_eq!(body["canCheck"], false, "a check is an admin's");
            assert_eq!(
                body["default"],
                json!({ "contents": "write", "metadata": "read", "pull_requests": "write" })
            );
            assert!(
                body["catalog"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|e| e["key"] == "administration")
            );
            let block = &body["hosts"][0];
            assert_eq!(block["host"], h.as_str());
            assert_eq!(block["narrowing"], "perRepo");
            assert_eq!(block["permissions"], full);
            assert_eq!(block["isDefault"], false);
            assert_eq!(block["repos"][0]["installation"], "95001");
            assert_eq!(block["installations"][0]["id"], "95001", "{block}");
            assert_eq!(block["repos"][0]["shortfall"], json!({}));
            assert_eq!(block["workspaceSecret"], Value::Null);
            assert_eq!(
                block["repos"][0]["lastToken"],
                Value::Null,
                "no token yet: nothing claimed"
            );
            let key = |k: &str| {
                block["keys"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["key"] == k)
                    .cloned()
                    .unwrap_or_default()
            };
            assert_eq!(key("administration")["held"], true);
            assert_eq!(key("organization_secrets")["scope"], "organization");

            // The other agent is on the default.
            let (_, body) =
                answer(get(State(state.clone()), as_admin.clone(), Path(b.clone())).await).await;
            assert_eq!(body["hosts"][0]["isDefault"], true);
            assert_eq!(
                body["hosts"][0]["permissions"],
                json!({ "contents": "write", "metadata": "read", "pull_requests": "write" })
            );

            // Each agent's git gets its own token: the set, met with what
            // the installation holds, and the default.
            let sb = state.secretbox().await.unwrap();
            let row = talaria_git_hosts::get_host(&state.pg, &h)
                .await
                .unwrap()
                .unwrap();
            let token = talaria_git_hosts::grant_login(&state.pg, &sb, &row, "acme/widgets", &a)
                .await
                .unwrap()
                .expect("a credential");
            let other = talaria_git_hosts::grant_login(&state.pg, &sb, &row, "acme/widgets", &b)
                .await
                .unwrap()
                .expect("a credential");
            assert_ne!(token.login.password, other.login.password);
            // Repo-scoped mints only: another live test's grants editor lists
            // every registered host's repositories, this one's included, with
            // an installation-wide token, which posts no body.
            let posted: Vec<Value> = bodies(&log, "POST", "/app/installations/95001/access_tokens")
                .into_iter()
                .filter_map(|b| serde_json::from_str::<Value>(&b).ok())
                .map(|b| b["permissions"].clone())
                .collect();
            assert_eq!(
                posted,
                [
                    full.clone(),
                    json!({ "contents": "write", "metadata": "read", "pull_requests": "write" }),
                ],
                "two agents, two mints"
            );

            // Narrowing takes the token back here and tells every process;
            // the other agent's token is untouched.
            let subscribe = RealtimeDeps::streams_only(&state.cfg.redis_url).subscribe;
            let mut heard = subscribe(talaria_git_hosts::revoke::TOKEN_REVOKE_TOPIC.into()).await;
            let take = json!({ "host": h, "from": full, "to": null });
            let (status, body) = put_as(&state, &as_admin, &a, take).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(body["isDefault"], true);
            let deleted = calls(&log, "DELETE", "/installation/token");
            assert!(
                deleted.contains(&format!("Bearer {}", token.login.password)),
                "the token was revoked on GitHub"
            );
            assert!(!deleted.contains(&format!("Bearer {}", other.login.password)));
            // The topic is every process's, and other live tests revoke on it
            // too: this agent's message is the one looked for.
            let message = tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let payload = heard.recv().await.expect("a message");
                    if let Some(b) = talaria_git_hosts::revoke::read_broadcast(&payload)
                        && b.agent == a
                    {
                        return b;
                    }
                }
            })
            .await
            .expect("the broadcast arrives");
            assert_eq!(message.host.as_deref(), Some(h.as_str()));
            assert_eq!(message.repo, None, "every repo of the agent on the host");

            cleanup(&state, &[&a, &b], &[&admin, &manager], &[&h]).await;
        }
    }
}
