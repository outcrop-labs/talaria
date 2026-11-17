// Which GitHub permissions each agent's token carries on each host:
// `agent_github_permissions`, one row per (agent, host), the permissions as a
// `{key: level}` object.
//
// THE AGENT'S WHOLE SET, PER HOST. A row is everything the agent's token may
// carry on that host, not something added to a base: any key GitHub knows,
// repository and organization keys alike, administration included, narrower
// or wider than the default. It applies to every repository the agent is
// granted there (`workbench_repos` stays the repo gate and carries no
// permission). A token is still minted per repository, filed under the
// agent, carrying the set met with what the installation holds, so the
// narrowing of one agent's set takes back that agent's tokens alone. No row
// is the default, [`DEFAULT_AGENT_PERMISSIONS`]: contents and pull requests
// to write, metadata to read, what the sandbox carried before sets were per
// agent.
//
// ADMINS ONLY decide (docs/PERMISSIONS.md, "Agent managers"): an agent's
// managers see what it holds, read-only. The route enforces it and checks
// each key against the adapter's catalog (talaria-github's `CATALOG`: a key
// and level GitHub knows, nothing more); this module writes what the route
// validated, through two doors only:
//   · `set_agent_host_permissions_cas`, the editor's write: from what the
//     admin read to what they chose (or back to the default), refused when
//     it changed meanwhile, with its `workbench.github_permissions` audit row
//     in the same transaction
//   · `merge_agent_host_permission`, an approval's write (a request names one
//     permission): raises one key, never lowers it, atomically, starting
//     from the default when the agent had no row
// Both take the (agent, host) advisory lock, so neither lands inside the
// other's read-then-write.
//
// WHAT IS READ FROM HERE besides the mint: the activation audit the holdings
// refresh writes when an installation accepts a permission an agent was
// already given, and the admin alert for agents whose permissions the
// installation has not accepted (talaria-alerts), both from the record and
// never from GitHub, and both about rows an admin set: an agent on the
// default was given nothing.

use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};

use talaria_audit::{AuditEntry, log_audit_in};
use talaria_git_host_types::{Level, PermissionSet};
pub use talaria_github::DEFAULT_AGENT_PERMISSIONS;

use crate::holdings::{HostHoldings, InstallationRow};

/// The set an agent carries on a host where no admin set one.
pub fn default_set() -> PermissionSet {
    PermissionSet::of(DEFAULT_AGENT_PERMISSIONS)
}

/// What an agent carries on a host, given its row: the row's set, or the
/// default when there is none.
pub fn effective(set: Option<PermissionSet>) -> PermissionSet {
    set.unwrap_or_else(default_set)
}

/// A stored set as it was set, or None when there is no row. A set that
/// cannot be read is an empty one (and said): no token is minted from it,
/// which is narrower than any guess at what an admin meant.
fn stored(agent_id: &str, host_id: &str, v: Option<Value>) -> Option<PermissionSet> {
    let v = v.filter(|v| !v.is_null())?;
    Some(PermissionSet::from_json(&v).unwrap_or_else(|e| {
        tracing::warn!(
            "[git-hosts] agent {agent_id}'s GitHub permissions on {host_id} cannot be read ({e}) — its token asks for nothing until an admin sets them again"
        );
        PermissionSet::new()
    }))
}

/// The set an admin gave `agent_id` on `host_id`, or None when none did (the
/// agent carries the default).
pub async fn set_on_host(
    pg: &PgPool,
    agent_id: &str,
    host_id: &str,
) -> Result<Option<PermissionSet>, String> {
    let row: Option<Value> = sqlx::query_scalar(
        "select permissions from agent_github_permissions \
         where agent_id = $1::uuid and host_id = $2",
    )
    .bind(agent_id)
    .bind(host_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("agent GitHub permissions read: {e}"))?;
    Ok(stored(agent_id, host_id, row))
}

/// What `agent_id`'s token may carry on `host_id`: its set, or the default.
pub async fn host_permissions(
    pg: &PgPool,
    agent_id: &str,
    host_id: &str,
) -> Result<PermissionSet, String> {
    Ok(effective(set_on_host(pg, agent_id, host_id).await?))
}

/// One agent's set on one host, as an admin set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHostPermissions {
    pub host_id: String,
    pub permissions: PermissionSet,
    /// The user who last set them, when they are still a user.
    pub updated_by: Option<String>,
    /// Epoch ms.
    pub updated_at_ms: i64,
}

/// Every host's row for `agent_id`, by host. A host with no row is one where
/// the agent carries the default.
pub async fn agent_permissions(
    pg: &PgPool,
    agent_id: &str,
) -> Result<Vec<AgentHostPermissions>, String> {
    let rows: Vec<(String, Value, Option<String>, i64)> = sqlx::query_as(
        "select host_id, permissions, updated_by::text, \
                (trunc(extract(epoch from updated_at) * 1000))::bigint \
         from agent_github_permissions where agent_id = $1::uuid order by host_id",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("agent GitHub permissions read: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(host_id, permissions, updated_by, updated_at_ms)| {
            let permissions = stored(agent_id, &host_id, Some(permissions)).unwrap_or_default();
            AgentHostPermissions {
                host_id,
                permissions,
                updated_by,
                updated_at_ms,
            }
        })
        .collect())
}

/// The advisory lock both writers take on (agent, host), inside their
/// transaction: a CAS's read and write, and a merge, never interleave, even
/// when there is no row yet for either to lock.
async fn lock(conn: &mut PgConnection, agent_id: &str, host_id: &str) -> Result<(), String> {
    sqlx::query("select pg_advisory_xact_lock(hashtext($1))")
        .bind(format!("agent_github_permissions:{agent_id}:{host_id}"))
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(|e| format!("agent GitHub permissions lock: {e}"))
}

/// What a compare-and-set did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cas {
    /// The agent now carries what was asked.
    Ok,
    /// It did not carry `from` when the write came: this is its row now
    /// (None: the default). The editor reads again, and nothing was written.
    Changed(Option<PermissionSet>),
}

/// Who changed an agent's permissions, and what the audit row says beside
/// the change.
pub struct PermissionsChange<'a> {
    /// The audit row's actor (talaria-session's `actor_of`).
    pub actor: &'a str,
    /// The user's id, kept as `updated_by`.
    pub user_id: Option<&'a str>,
    /// The agent's name, the audit row's label.
    pub agent_label: &'a str,
    /// Merged into the row's after-data: what the admin was told beside the
    /// write.
    pub note: Value,
}

/// A set as an audit row records it: what the agent carries, and whether
/// that is the default.
fn audit_set(host_id: &str, set: Option<&PermissionSet>) -> Value {
    json!({
        "host": host_id,
        "permissions": set.cloned().unwrap_or_else(default_set).to_json(),
        "default": set.is_none(),
    })
}

/// THE EDITOR'S WRITE: set what `agent_id` carries on `host_id` to `to`
/// (None: back to the default, and the row goes), if it carries `from` now
/// (what the admin read, the default included). The comparison is of what
/// the agent carries, so a row equal to the default and no row are one
/// read. The `workbench.github_permissions` audit row is written in the same
/// transaction, so a write that landed always has its row. Revoking what a
/// narrowing took away is the caller's ([`crate::revoke`]), after this
/// commits.
pub async fn set_agent_host_permissions_cas(
    pg: &PgPool,
    agent_id: &str,
    host_id: &str,
    from: &PermissionSet,
    to: Option<&PermissionSet>,
    change: &PermissionsChange<'_>,
) -> Result<Cas, String> {
    let write = |e: sqlx::Error| format!("agent GitHub permissions write: {e}");
    let mut tx = pg.begin().await.map_err(write)?;
    lock(&mut tx, agent_id, host_id).await?;
    let current: Option<Value> = sqlx::query_scalar(
        "select permissions from agent_github_permissions \
         where agent_id = $1::uuid and host_id = $2",
    )
    .bind(agent_id)
    .bind(host_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| format!("agent GitHub permissions read: {e}"))?;
    let current = stored(agent_id, host_id, current);
    if &effective(current.clone()) != from {
        return Ok(Cas::Changed(current));
    }
    match to {
        None => {
            sqlx::query(
                "delete from agent_github_permissions where agent_id = $1::uuid and host_id = $2",
            )
            .bind(agent_id)
            .bind(host_id)
            .execute(&mut *tx)
            .await
            .map_err(write)?;
        }
        Some(to) => {
            sqlx::query(
                "insert into agent_github_permissions (agent_id, host_id, permissions, updated_by) \
                 values ($1::uuid, $2, $3, $4::uuid) \
                 on conflict (agent_id, host_id) do update set \
                   permissions = excluded.permissions, updated_by = excluded.updated_by, \
                   updated_at = now()",
            )
            .bind(agent_id)
            .bind(host_id)
            .bind(to.to_json())
            .bind(change.user_id)
            .execute(&mut *tx)
            .await
            .map_err(write)?;
        }
    }
    let mut after = audit_set(host_id, to);
    if let (Some(after), Some(note)) = (after.as_object_mut(), change.note.as_object()) {
        for (k, v) in note {
            after.insert(k.clone(), v.clone());
        }
    }
    log_audit_in(
        &mut tx,
        AuditEntry {
            actor: change.actor,
            action: "workbench.github_permissions",
            target_type: "agent",
            target_id: Some(agent_id),
            target_label: Some(change.agent_label),
            before: Some(audit_set(host_id, current.as_ref())),
            after: Some(after),
        },
    )
    .await
    .map_err(write)?;
    tx.commit().await.map_err(write)?;
    Ok(Cas::Ok)
}

/// AN APPROVAL'S WRITE: give `agent_id` permission `key` at `level` on
/// `host_id`, in the caller's transaction — one statement, which adds the
/// key or raises it and never lowers a level the agent already holds. An
/// agent with no row starts from the default, so an approval adds to what
/// it carried rather than replacing it. It answers what the agent carries
/// after. The caller validated the key against the catalog and writes its
/// own decision's audit row beside it.
pub async fn merge_agent_host_permission(
    tx: &mut PgConnection,
    agent_id: &str,
    host_id: &str,
    key: &str,
    level: Level,
    user_id: Option<&str>,
) -> Result<PermissionSet, String> {
    lock(tx, agent_id, host_id).await?;
    let seed = default_set().union(&PermissionSet::of(&[(key, level)]));
    let after: Value = sqlx::query_scalar(
        "insert into agent_github_permissions (agent_id, host_id, permissions, updated_by) \
         values ($1::uuid, $2, $6::jsonb, $5::uuid) \
         on conflict (agent_id, host_id) do update set \
           permissions = agent_github_permissions.permissions || jsonb_build_object($3::text, \
             case when array_position(array['read','write','admin'], \
                                      agent_github_permissions.permissions->>$3::text) \
                       >= array_position(array['read','write','admin'], $4::text) \
                  then agent_github_permissions.permissions->>$3::text \
                  else $4::text end), \
           updated_by = excluded.updated_by, updated_at = now() \
         returning permissions",
    )
    .bind(agent_id)
    .bind(host_id)
    .bind(key)
    .bind(level.as_str())
    .bind(user_id)
    .bind(seed.to_json())
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| format!("agent GitHub permission merge: {e}"))?;
    Ok(stored(agent_id, host_id, Some(after)).unwrap_or_default())
}

/// THE CREDENTIAL DOOR'S RE-CHECK ([`crate::grant_login`]): what `agent_id`
/// may carry on `repo` of `host_id` now. Its set on the host (or the
/// default), while it still holds the grant and is an enabled Developer
/// Agent; None when either went.
pub async fn recheck(
    pg: &PgPool,
    agent_id: &str,
    host_id: &str,
    repo: &str,
) -> Result<Option<PermissionSet>, String> {
    let row: Option<(bool, Option<Value>)> = sqlx::query_as(
        "select d.developer and d.enabled, p.permissions \
         from agent_defs d \
         join workbench_repos w on w.agent_id = d.id and w.host_id = $2 and w.repo = $3 \
         left join agent_github_permissions p on p.agent_id = d.id and p.host_id = $2 \
         where d.id = $1::uuid",
    )
    .bind(agent_id)
    .bind(host_id)
    .bind(repo)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("agent grant re-read: {e}"))?;
    Ok(match row {
        Some((true, permissions)) => Some(effective(stored(agent_id, host_id, permissions))),
        _ => None,
    })
}

// ── What the record says about them ──────────────────────────────────────────

/// One agent's set on one host, as an admin set it, with the repos it holds
/// there: what the activation audit and the dormant-permissions alert read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSet {
    pub agent_id: String,
    /// The agent's display name.
    pub agent_name: String,
    pub host_id: String,
    pub permissions: PermissionSet,
    /// The repos it is granted on the host, by name.
    pub repos: Vec<String>,
}

/// Every set an admin gave an agent, on every host. An agent on the default
/// is not here: it was given nothing.
pub async fn agent_sets(pg: &PgPool) -> Result<Vec<AgentSet>, String> {
    let rows: Vec<(String, String, String, Value, Vec<String>)> = sqlx::query_as(
        "select p.agent_id::text, d.display_name, p.host_id, p.permissions, \
                coalesce(array_agg(w.repo order by w.repo) filter (where w.repo is not null), \
                         '{}'::text[]) \
         from agent_github_permissions p \
         join agent_defs d on d.id = p.agent_id \
         left join workbench_repos w on w.agent_id = p.agent_id and w.host_id = p.host_id \
         where p.permissions <> '{}'::jsonb \
         group by p.agent_id, d.display_name, p.host_id, p.permissions \
         order by d.display_name, p.host_id",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("agent GitHub permissions read: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(agent_id, agent_name, host_id, permissions, repos)| {
            let permissions = stored(&agent_id, &host_id, Some(permissions)).unwrap_or_default();
            AgentSet {
                agent_id,
                agent_name,
                host_id,
                permissions,
                repos,
            }
        })
        .collect())
}

/// Whether `repo` (`owner/name`) lives on `account`: a GitHub App is
/// installed per account, and an installation's repos are its account's
/// (GitHub names ignore case).
pub fn on_account(repo: &str, account: &str) -> bool {
    repo.split('/')
        .next()
        .is_some_and(|owner| !owner.is_empty() && owner.eq_ignore_ascii_case(account))
}

/// The installation a token for `repo` is minted on, as far as the record
/// can tell without GitHub: the only one the host uses, else the one
/// installed on the repo's owner ([`on_account`]). None when no selected
/// installation is on that account.
pub fn installation_for_repo<'a>(
    selected: &[String],
    rows: &'a [InstallationRow],
    repo: &str,
) -> Option<&'a InstallationRow> {
    if let [only] = selected {
        return rows.iter().find(|r| &r.installation_id == only);
    }
    rows.iter()
        .filter(|r| selected.contains(&r.installation_id))
        .find(|r| on_account(repo, &r.account))
}

/// An agent whose set on a host names permissions the installation its repos
/// are minted on has not accepted: given in Talaria, carried by no token yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dormant {
    pub agent_id: String,
    pub agent_name: String,
    pub host_id: String,
    pub host_name: String,
    /// What it was given that the installation does not hold.
    pub missing: PermissionSet,
    /// The repos whose tokens go without it.
    pub repos: Vec<String>,
}

/// Every agent given a permission on a host whose installation has not
/// accepted it, from the record alone (`holdings`, as `recorded_holdings`
/// reads it). An installation whose holdings were never read is not counted
/// here: that is the "unknown" alert's, never a dormant grant. An agent with
/// no repo on the host carries nothing anywhere, so it is not counted
/// either.
pub fn dormant(holdings: &[HostHoldings], sets: &[AgentSet]) -> Vec<Dormant> {
    let mut out = Vec::new();
    for e in sets {
        let Some(h) = holdings.iter().find(|h| h.host.id == e.host_id) else {
            continue;
        };
        let mut missing = PermissionSet::new();
        let mut repos = Vec::new();
        for repo in &e.repos {
            let held =
                installation_for_repo(&h.selected, &h.rows, repo).and_then(|r| r.held.as_ref());
            if let Some(held) = held {
                let gap = e.permissions.missing_from(held);
                if !gap.is_empty() {
                    missing = missing.union(&gap);
                    repos.push(repo.clone());
                }
            }
        }
        if !missing.is_empty() {
            out.push(Dormant {
                agent_id: e.agent_id.clone(),
                agent_name: e.agent_name.clone(),
                host_id: e.host_id.clone(),
                host_name: h.host.name.clone(),
                missing,
                repos,
            });
        }
    }
    out
}

/// What of an agent's `set` an installation's change from `before` to `now`
/// made effective: each permission it lacked before, at the level given,
/// and holds now.
pub fn activated(
    set: &PermissionSet,
    before: &PermissionSet,
    now: &PermissionSet,
) -> PermissionSet {
    let still = set.missing_from(now);
    set.missing_from(before).without(|k| still.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::HostRow;
    use talaria_git_host_types::HostKind;

    fn set(pairs: &[(&str, Level)]) -> PermissionSet {
        PermissionSet::of(pairs)
    }

    fn row(id: &str, account: &str, held: Option<PermissionSet>) -> InstallationRow {
        InstallationRow {
            host_id: "github".into(),
            installation_id: id.into(),
            account: account.into(),
            held,
            checked_at_ms: Some(1),
            ..InstallationRow::default()
        }
    }

    fn holdings(selected: &[&str], rows: Vec<InstallationRow>) -> HostHoldings {
        HostHoldings {
            host: HostRow {
                id: "github".into(),
                kind: HostKind::Github,
                name: "GitHub".into(),
                base_url: "https://github.com".into(),
                api_url: None,
                auth: json!({}),
                repo_creation_orgs: vec![],
                created_at: String::new(),
                updated_at: String::new(),
            },
            selected: selected.iter().map(|s| s.to_string()).collect(),
            rows,
        }
    }

    fn agent_set(agent: &str, permissions: PermissionSet, repos: &[&str]) -> AgentSet {
        AgentSet {
            agent_id: agent.into(),
            agent_name: format!("Agent {agent}"),
            host_id: "github".into(),
            permissions,
            repos: repos.iter().map(|r| r.to_string()).collect(),
        }
    }

    #[test]
    fn a_repo_is_minted_on_the_installation_of_its_owner() {
        let rows = vec![row("91", "acme", None), row("92", "Beta", None)];
        let selected = ["91".to_string(), "92".to_string()];
        let on = |repo: &str| {
            installation_for_repo(&selected, &rows, repo).map(|r| r.installation_id.as_str())
        };
        assert_eq!(on("acme/widgets"), Some("91"));
        assert_eq!(on("beta/app"), Some("92"), "GitHub names ignore case");
        assert_eq!(on("gamma/app"), None);
        // One installation: every repo is its.
        assert_eq!(
            installation_for_repo(&["91".to_string()], &rows, "gamma/app")
                .map(|r| r.installation_id.as_str()),
            Some("91")
        );
    }

    #[test]
    fn a_permission_the_installation_has_not_accepted_is_dormant_and_unknown_is_not() {
        let wf = set(&[("workflows", Level::Write)]);
        let without = set(&[("contents", Level::Write), ("metadata", Level::Read)]);
        let with = without.union(&wf);
        let h = [holdings(
            &["91", "92", "93"],
            vec![
                row("91", "acme", Some(without.clone())),
                row("92", "beta", Some(with.clone())),
                // 93 was never read: unknown, the other alert's.
                row("93", "gamma", None),
            ],
        )];
        let found = dormant(
            &h,
            &[
                agent_set("a1", wf.clone(), &["acme/site", "beta/app"]),
                agent_set("a2", wf.clone(), &["beta/app"]),
                agent_set("a3", wf.clone(), &["gamma/x"]),
                // Nothing granted on the host: no token carries anything.
                agent_set("a4", wf.clone(), &[]),
            ],
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].agent_id, "a1");
        assert_eq!(found[0].missing, wf);
        assert_eq!(found[0].repos, ["acme/site"]);
        assert_eq!(found[0].host_name, "GitHub");
        // Another host's agent is not this host's.
        let mut elsewhere = agent_set("a5", wf, &["acme/site"]);
        elsewhere.host_id = "forge".into();
        assert!(dormant(&h, &[elsewhere]).is_empty());
    }

    #[test]
    fn a_permission_is_activated_when_the_installation_starts_to_hold_it() {
        let wf = set(&[("workflows", Level::Write)]);
        let base = set(&[("contents", Level::Write), ("metadata", Level::Read)]);
        assert_eq!(activated(&wf, &base, &base.union(&wf)), wf);
        // Held before: nothing new.
        assert!(activated(&wf, &base.union(&wf), &base.union(&wf)).is_empty());
        // Gained at a level below the one given: still not effective.
        let read = base.union(&set(&[("workflows", Level::Read)]));
        assert!(activated(&wf, &base, &read).is_empty());
        // Something else gained.
        assert!(activated(&wf, &base, &base.union(&set(&[("checks", Level::Read)]))).is_empty());
    }

    #[test]
    fn no_row_is_the_default_and_a_row_nobody_can_read_is_nothing() {
        assert_eq!(stored("a", "h", None), None);
        assert_eq!(stored("a", "h", Some(Value::Null)), None);
        assert_eq!(effective(None), default_set());
        assert_eq!(
            default_set(),
            set(&[
                ("contents", Level::Write),
                ("pull_requests", Level::Write),
                ("metadata", Level::Read),
            ])
        );
        assert_eq!(
            stored("a", "h", Some(json!({ "workflows": "sometimes" }))),
            Some(PermissionSet::new())
        );
        let set_by_admin = set(&[("administration", Level::Write)]);
        assert_eq!(
            stored("a", "h", Some(json!({ "administration": "write" }))),
            Some(set_by_admin.clone())
        );
        // A set is the whole set: nothing of the default is added to it.
        assert_eq!(effective(Some(set_by_admin.clone())), set_by_admin);
    }
}
