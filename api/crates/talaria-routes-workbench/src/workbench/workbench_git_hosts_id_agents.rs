// /api/workbench/git-hosts/{id}/agents. The host card's "Agents' tokens":
// what an installation accepting a permission means for the agents, which is
// not the same thing. GET → each agent with a GitHub permission set or a
// repo grant on the host: its set (or the default), what its token carries
// (the set met with what the installations its repos are minted on hold, as
// the record has it), and its last token (the newest `git.credential` or
// `git.credential_refused` row on the host: what GitHub put on it, or why
// there was none). Beside them, `unclaimed`: what the installations hold,
// metadata aside, that no Developer agent's set names — accepted on GitHub,
// carried by no agent, because a grant is per agent and admin-only. Read from
// the record and the trail, never GitHub. requireAdmin, like the rest of the
// host plane.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_error::internal;
use talaria_git_hosts::credential::{self, LastToken};
use talaria_git_hosts::holdings::{InstallationRow, installation_rows, tracked_installations};
use talaria_git_hosts::permissions::{default_set, effective};
use talaria_git_hosts::token_view::{Credential, GrantCredential, credential_of};
use talaria_git_hosts::{self as hosts, HostRow, Narrowing, PermissionSet};
use talaria_session::require_admin;
use talaria_state::AppState;

use super::git_host_or_404;

/// One agent on the host, as the record has it.
struct HostAgent {
    id: String,
    name: String,
    /// Its Developer switch is on and it is enabled: only such an agent's
    /// git gets a credential at all.
    developer: bool,
    /// The set an admin gave it here, or None for the default.
    set: Option<PermissionSet>,
    repos: Vec<String>,
}

/// Every agent with a set or a repo grant on `host_id`, by name.
async fn host_agents(pg: &sqlx::PgPool, host_id: &str) -> Result<Vec<HostAgent>, sqlx::Error> {
    let rows: Vec<(String, String, bool, Option<Value>, Vec<String>)> = sqlx::query_as(
        "select d.id::text, d.display_name, d.developer and d.enabled, p.permissions, \
                coalesce((select array_agg(w.repo order by w.repo) from workbench_repos w \
                           where w.agent_id = d.id and w.host_id = $1), '{}'::text[]) \
         from agent_defs d \
         left join agent_github_permissions p on p.agent_id = d.id and p.host_id = $1 \
         where p.agent_id is not null \
            or exists (select 1 from workbench_repos w where w.agent_id = d.id and w.host_id = $1) \
         order by d.display_name, d.id",
    )
    .bind(host_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, developer, permissions, repos)| {
            // A set that cannot be read asks for nothing, as the mint reads it.
            let set = permissions.filter(|v| !v.is_null()).map(|v| {
                PermissionSet::from_json(&v).unwrap_or_else(|e| {
                    tracing::warn!(
                        "[workbench/git-hosts] agent {id}'s GitHub permissions on {host_id} cannot be read ({e})"
                    );
                    PermissionSet::new()
                })
            });
            HostAgent {
                id,
                name,
                developer,
                set,
                repos,
            }
        })
        .collect())
}

/// What `agent`'s token carries on `row`, as far as the record can tell: for
/// each repo, its set met with what the installation that mints the repo's
/// token holds (talaria-git-hosts' `GrantCredential::carries`), and what
/// every one of those carries; with no repo, what every installation the
/// host uses would give. None when no installation involved was read.
fn carries(
    row: &HostRow,
    agent: &HostAgent,
    selected: &[String],
    installations: &[InstallationRow],
) -> Option<PermissionSet> {
    let each: Vec<PermissionSet> = if agent.repos.is_empty() {
        let set = effective(agent.set.clone());
        installations
            .iter()
            .filter(|r| selected.contains(&r.installation_id))
            .filter_map(|r| {
                GrantCredential {
                    host_id: row.id.clone(),
                    host_name: row.name.clone(),
                    kind: row.kind,
                    repo: String::new(),
                    api: None,
                    credential: Credential::AppToken {
                        set: set.clone(),
                        is_default: agent.set.is_none(),
                        installation: Some(r.installation_id.clone()),
                        held: r.held.clone(),
                    },
                }
                .carries()
            })
            .collect()
    } else {
        agent
            .repos
            .iter()
            .filter_map(|repo| {
                credential_of(row, repo, agent.set.as_ref(), selected, installations, None)
                    .carries()
            })
            .collect()
    };
    each.into_iter().reduce(|a, b| a.meet(&b))
}

/// What the host's installations hold, metadata aside, that no Developer
/// agent's set names (an agent on the default names the default). None when
/// no installation was read.
fn unclaimed(
    agents: &[HostAgent],
    selected: &[String],
    installations: &[InstallationRow],
) -> Option<PermissionSet> {
    let held: Vec<&PermissionSet> = installations
        .iter()
        .filter(|r| selected.contains(&r.installation_id))
        .filter_map(|r| r.held.as_ref())
        .collect();
    if held.is_empty() {
        return None;
    }
    let all = held
        .into_iter()
        .fold(PermissionSet::new(), |acc, h| acc.union(h));
    let named = agents
        .iter()
        .filter(|a| a.developer)
        .map(|a| effective(a.set.clone()))
        .fold(PermissionSet::new(), |acc, s| acc.union(&s));
    Some(all.without(|k| k == "metadata" || named.contains(k)))
}

pub async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    require_admin(&state, &headers).await?;
    let row = git_host_or_404(&state, &id).await?;
    let agents = match host_agents(&state.pg, &row.id).await {
        Ok(a) => a,
        Err(e) => return Ok(internal("[workbench/git-hosts] agents read failed", e)),
    };
    let narrowing = hosts::narrowing_of(&row);
    let selected = tracked_installations(&row);
    let installations = installation_rows(&state.pg, &row.id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("[workbench/git-hosts] {e}");
            Vec::new()
        });
    let ids: Vec<String> = agents.iter().map(|a| a.id.clone()).collect();
    let last: Vec<LastToken> = credential::last_tokens(&state.pg, &ids)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("[workbench/git-hosts] {e}");
            Vec::new()
        });
    let per_repo = matches!(narrowing, Narrowing::PerRepo);
    let wire: Vec<Value> = agents
        .iter()
        .map(|a| {
            let set = per_repo.then(|| effective(a.set.clone()));
            // Newest first: the first row of this agent on this host.
            let newest = last
                .iter()
                .find(|t| t.agent_id == a.id && t.host_id == row.id)
                .map(|t| t.wire(set.as_ref()));
            json!({
                "agentId": a.id,
                "name": a.name,
                "developer": a.developer,
                "isDefault": per_repo.then_some(a.set.is_none()),
                "set": set.as_ref().map(PermissionSet::to_json),
                "carries": if per_repo {
                    carries(&row, a, &selected, &installations).map(|c| c.to_json())
                } else {
                    None
                },
                "repos": a.repos,
                "lastToken": newest,
            })
        })
        .collect();
    Ok(Json(json!({
        "host": row.id,
        "narrowing": narrowing.wire(),
        "default": default_set().to_json(),
        "agents": wire,
        "unclaimed": if per_repo {
            unclaimed(&agents, &selected, &installations).map(|u| u.to_json())
        } else {
            None
        },
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_git_hosts::{HostKind, Level};

    fn set(pairs: &[(&str, Level)]) -> PermissionSet {
        PermissionSet::of(pairs)
    }

    fn host() -> HostRow {
        HostRow {
            id: "gh".into(),
            kind: HostKind::Github,
            name: "GitHub".into(),
            base_url: "https://github.com".into(),
            api_url: None,
            auth: json!({ "mode": "app", "app": { "appId": "7", "installationIds": ["91"], "privateKeyEnc": "sealed" } }),
            repo_creation_orgs: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn installation(held: PermissionSet) -> Vec<InstallationRow> {
        vec![InstallationRow {
            host_id: "gh".into(),
            installation_id: "91".into(),
            account: "acme".into(),
            held: Some(held),
            checked_at_ms: Some(1),
            ..InstallationRow::default()
        }]
    }

    fn agent(id: &str, developer: bool, given: Option<PermissionSet>, repos: &[&str]) -> HostAgent {
        HostAgent {
            id: id.into(),
            name: id.into(),
            developer,
            set: given,
            repos: repos.iter().map(|r| r.to_string()).collect(),
        }
    }

    #[test]
    fn what_the_installation_holds_and_no_developer_set_names_is_unclaimed() {
        let held = set(&[
            ("actions", Level::Write),
            ("contents", Level::Write),
            ("metadata", Level::Read),
            ("pull_requests", Level::Write),
            ("workflows", Level::Write),
        ]);
        let rows = installation(held.clone());
        let selected = ["91".to_string()];
        // One agent on the default: workflows and actions are held and
        // carried by no one.
        let agents = vec![agent("a1", true, None, &["acme/widgets"])];
        assert_eq!(
            unclaimed(&agents, &selected, &rows),
            Some(set(&[
                ("actions", Level::Write),
                ("workflows", Level::Write)
            ]))
        );
        // A non-Developer agent's set claims nothing: its git gets nothing.
        let wf = set(&[("workflows", Level::Write)]);
        let agents = vec![
            agent("a1", true, None, &["acme/widgets"]),
            agent("a2", false, Some(wf.clone()), &["acme/widgets"]),
        ];
        assert_eq!(
            unclaimed(&agents, &selected, &rows),
            Some(set(&[
                ("actions", Level::Write),
                ("workflows", Level::Write)
            ]))
        );
        // Granted to a Developer agent, workflows is claimed.
        let agents = vec![
            agent("a1", true, None, &["acme/widgets"]),
            agent(
                "a3",
                true,
                Some(default_set().union(&wf)),
                &["acme/widgets"],
            ),
        ];
        assert_eq!(
            unclaimed(&agents, &selected, &rows),
            Some(set(&[("actions", Level::Write)]))
        );
        // Nothing read: unknown, never "nothing".
        assert_eq!(unclaimed(&agents, &selected, &[]), None);
    }

    #[test]
    fn an_agents_token_carries_its_set_met_with_what_is_held() {
        let held = set(&[
            ("contents", Level::Write),
            ("metadata", Level::Read),
            ("pull_requests", Level::Write),
        ]);
        let rows = installation(held.clone());
        let selected = ["91".to_string()];
        let row = host();
        // The default, all of it held.
        let a = agent("a1", true, None, &["acme/widgets"]);
        assert_eq!(carries(&row, &a, &selected, &rows), Some(default_set()));
        // Given workflows the installation lacks: the rest.
        let wf = set(&[("workflows", Level::Write)]);
        let b = agent(
            "a2",
            true,
            Some(default_set().union(&wf)),
            &["acme/widgets"],
        );
        assert_eq!(carries(&row, &b, &selected, &rows), Some(default_set()));
        // No repo: what the installation would give.
        let c = agent("a3", true, Some(wf.clone()), &[]);
        assert_eq!(
            carries(&row, &c, &selected, &rows),
            Some(PermissionSet::new())
        );
        // Nothing read: unknown.
        assert_eq!(carries(&row, &a, &selected, &[]), None);
    }
}
