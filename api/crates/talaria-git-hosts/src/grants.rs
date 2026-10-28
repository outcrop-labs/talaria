// Which agent may work which repository, on which host — and the branch law
// each grant carries, and the branch each repository's pull requests target.
//
// A grant is (agent, host, repo): connecting a host grants nothing to anyone
// by itself, and the same `owner/name` on two hosts is two repositories. The
// agents' tools take a repo and, when it is ambiguous, the host
// (`resolve_grant`).
//
// THE BRANCH LAW: work lives on a branch, a human merges. The DEFAULT
// (columns null/'branches_only') already enforces that — an agent with a
// grant and no configuration may push any branch except the base.
// Configuration narrows or widens: name the base explicitly (a repo whose
// default branch is not the integration target), require a prefix for the
// agent's branches, or explicitly trust an agent with the base ('free'). A
// host's own branch protection enforces the same posture where it can; this
// is the platform's OWN gate, which works for every repo on every host.

use serde::Serialize;
use sqlx::PgPool;

use talaria_git_host_types::GitHost;

use crate::Host;

/// One repository on one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRef {
    /// The host's id (`git_hosts.id`); `host` on the wire, as the agents'
    /// tools name it.
    #[serde(rename = "host")]
    pub host_id: String,
    pub repo: String,
}

/// The repos an agent was actually granted, across every host.
pub async fn granted_repos(pg: &PgPool, agent_id: &str) -> Vec<RepoRef> {
    sqlx::query_as::<_, (String, String)>(
        "select host_id, repo from workbench_repos where agent_id = $1::uuid order by host_id, repo",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
    .map(|rows| {
        rows.into_iter()
            .map(|(host_id, repo)| RepoRef { host_id, repo })
            .collect()
    })
    .unwrap_or_default()
}

/// The one grant an agent's `repo` (and, when given, `host`) names. A repo
/// granted on several hosts needs the host; a repo granted nowhere is
/// refused with where to look.
pub async fn resolve_grant(
    pg: &PgPool,
    agent_id: &str,
    repo: &str,
    host: Option<&str>,
) -> Result<RepoRef, String> {
    let repo = repo.trim();
    let host = host.map(str::trim).filter(|h| !h.is_empty());
    let grants = granted_repos(pg, agent_id).await;
    let on_host = |g: &&RepoRef| host.is_none_or(|h| g.host_id == h);
    let exact: Vec<&RepoRef> = grants
        .iter()
        .filter(on_host)
        .filter(|g| g.repo == repo)
        .collect();
    let matches = if exact.is_empty() {
        grants
            .iter()
            .filter(on_host)
            .filter(|g| g.repo.eq_ignore_ascii_case(repo))
            .collect()
    } else {
        exact
    };
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(match host {
            Some(h) => format!(
                "repo \"{repo}\" on host \"{h}\" is not granted to you — list_repos shows yours"
            ),
            None => format!("repo \"{repo}\" is not granted to you — list_repos shows yours"),
        }),
        many => Err(format!(
            "repo \"{repo}\" is granted to you on more than one host ({}) — pass host",
            many.iter()
                .map(|g| g.host_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Atomic grant replace — a crash can never leave the agent grantless by
/// accident. `keep` is capped at 100.
pub async fn set_granted_repos(
    pg: &PgPool,
    agent_id: &str,
    keep: &[RepoRef],
) -> Result<(), String> {
    let keep = &keep[..keep.len().min(100)];
    let hosts: Vec<String> = keep.iter().map(|g| g.host_id.clone()).collect();
    let repos: Vec<String> = keep.iter().map(|g| g.repo.clone()).collect();
    let mut tx = pg
        .begin()
        .await
        .map_err(|e| format!("workbench grant tx: {e}"))?;
    // SET semantics, not replace-by-delete: rows that survive keep their
    // hygiene rules, rows that leave go away, rows that arrive start at the
    // defaults. A delete-everything write would wipe an admin's branch rules
    // on every chip toggle.
    sqlx::query(
        "delete from workbench_repos w where w.agent_id = $1::uuid \
         and not exists (select 1 from unnest($2::text[], $3::text[]) k(host_id, repo) \
                         where k.host_id = w.host_id and k.repo = w.repo)",
    )
    .bind(agent_id)
    .bind(&hosts)
    .bind(&repos)
    .execute(&mut *tx)
    .await
    .map_err(|e| format!("workbench grant clear: {e}"))?;
    if !keep.is_empty() {
        sqlx::query(
            "insert into workbench_repos (agent_id, host_id, repo) \
             select $1::uuid, k.host_id, k.repo from unnest($2::text[], $3::text[]) k(host_id, repo) \
             on conflict do nothing",
        )
        .bind(agent_id)
        .bind(&hosts)
        .bind(&repos)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("workbench grant write: {e}"))?;
    }
    tx.commit()
        .await
        .map_err(|e| format!("workbench grant commit: {e}"))
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRule {
    #[serde(rename = "host")]
    pub host_id: String,
    pub repo: String,
    /// The integration branch — never pushable in branches_only mode. Null
    /// means "the repo's default branch", resolved live at check time.
    pub base_branch: Option<String>,
    /// 'branches_only' (the default posture) or 'free'.
    pub push_mode: String,
    /// When set, the agent's branches must start with this prefix.
    pub branch_prefix: Option<String>,
}

type RuleRow = (String, String, Option<String>, String, Option<String>);

fn rule_of(r: RuleRow) -> RepoRule {
    RepoRule {
        host_id: r.0,
        repo: r.1,
        base_branch: r.2,
        push_mode: r.3,
        branch_prefix: r.4,
    }
}

pub async fn repo_rules(pg: &PgPool, agent_id: &str) -> Vec<RepoRule> {
    sqlx::query_as::<_, RuleRow>(
        "select host_id, repo, base_branch, push_mode, branch_prefix from workbench_repos \
         where agent_id = $1::uuid order by host_id, repo",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
    .map(|rows| rows.into_iter().map(rule_of).collect())
    .unwrap_or_default()
}

/// The one-row read the push check needs. None = no grant, which the caller
/// answers exactly like the credential route's no-credential case.
pub async fn repo_rule(pg: &PgPool, agent_id: &str, host_id: &str, repo: &str) -> Option<RepoRule> {
    sqlx::query_as::<_, RuleRow>(
        "select host_id, repo, base_branch, push_mode, branch_prefix from workbench_repos \
         where agent_id = $1::uuid and host_id = $2 and repo = $3",
    )
    .bind(agent_id)
    .bind(host_id)
    .bind(repo)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
    .map(rule_of)
}

/// Write one grant's rules. The repo must already be granted (the route
/// validates); values are trusted to the route's own validation.
pub async fn set_repo_rule(pg: &PgPool, agent_id: &str, rule: &RepoRule) -> Result<(), String> {
    sqlx::query(
        "update workbench_repos set base_branch = $4, push_mode = $5, branch_prefix = $6 \
         where agent_id = $1::uuid and host_id = $2 and repo = $3",
    )
    .bind(agent_id)
    .bind(&rule.host_id)
    .bind(&rule.repo)
    .bind(&rule.base_branch)
    .bind(if rule.push_mode == "free" {
        "free"
    } else {
        "branches_only"
    })
    .bind(&rule.branch_prefix)
    .execute(pg)
    .await
    .map_err(|e| format!("repo rule write: {e}"))?;
    Ok(())
}

/// Branches no agent pushes under `branches_only`, whatever the rule names
/// as its base. A repo whose flow base is `rc` (this repository's own model)
/// still has a `main` that promotion owns; a rule that only refused the
/// configured base let a push to `main` through. `free` is the operator's
/// explicit opt-out and keeps its meaning.
pub const PROTECTED_BRANCHES: [&str; 3] = ["main", "master", "rc"];

/// The decision the pre-push hook and the brief both read. Pure: given the
/// rule, the repo's resolved default branch (for a null base_branch), and a
/// ref being pushed, allow or refuse with a sentence for the agent.
///
/// Refused under `branches_only`: the configured base, the repo's default
/// branch (even when a base is configured — the default is where the repo's
/// own promotion lands), and [`PROTECTED_BRANCHES`].
///
/// THIS IS A CLIENT-SIDE GATE. It answers the rendered pre-push hook inside
/// the sandbox, and git skips a pre-push hook on `--no-verify`. The base
/// branch is only actually safe when the repository's own branch protection
/// says so. This rule is the agent's instruction and the operator's log
/// line, not the lock on the door.
pub fn push_allowed(rule: &RepoRule, default_branch: &str, ref_name: &str) -> Result<(), String> {
    if rule.push_mode == "free" {
        return Ok(());
    }
    let branch = ref_name.trim_start_matches("refs/heads/");
    let base = rule.base_branch.as_deref().unwrap_or(default_branch);
    if branch == base {
        return Err(format!(
            "{base} is the base branch for {} — your work belongs on your own branch and a human merges it",
            rule.repo
        ));
    }
    if !default_branch.is_empty() && branch == default_branch {
        return Err(format!(
            "{default_branch} is the default branch of {} — your work belongs on your own branch and a human merges it",
            rule.repo
        ));
    }
    if PROTECTED_BRANCHES.contains(&branch) {
        return Err(format!(
            "{branch} is a protected branch of {} — your work belongs on your own branch and a human merges it",
            rule.repo
        ));
    }
    if let Some(prefix) = rule.branch_prefix.as_deref().filter(|p| !p.is_empty())
        && !branch.starts_with(prefix)
    {
        return Err(format!(
            "branches for {} must start with \"{prefix}\" — got \"{branch}\"",
            rule.repo
        ));
    }
    Ok(())
}

// ── Per-repo git flow (the branch PRs target) ────────────────────────────────

/// workbench_repo_flow's row — `{host, repo, baseBranch}` on the wire, a
/// null branch meaning "not set".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFlow {
    #[serde(rename = "host")]
    pub host_id: String,
    pub repo: String,
    pub base_branch: Option<String>,
}

pub async fn repo_flow(pg: &PgPool, host_id: &str, repo: &str) -> Result<RepoFlow, String> {
    let row: Option<Option<String>> = sqlx::query_scalar(
        "select base_branch from workbench_repo_flow where host_id = $1 and repo = $2",
    )
    .bind(host_id)
    .bind(repo)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("repo flow read: {e}"))?;
    Ok(RepoFlow {
        host_id: host_id.to_string(),
        repo: repo.to_string(),
        base_branch: row.flatten(),
    })
}

pub async fn list_repo_flows(pg: &PgPool) -> Result<Vec<RepoFlow>, String> {
    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "select host_id, repo, base_branch from workbench_repo_flow order by host_id, repo",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("repo flows read: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(host_id, repo, base_branch)| RepoFlow {
            host_id,
            repo,
            base_branch,
        })
        .collect())
}

/// Upsert one repo's flow. None keeps the current value, Some(None) clears
/// it.
pub async fn set_repo_flow(
    pg: &PgPool,
    host_id: &str,
    repo: &str,
    base_branch: Option<Option<String>>,
) -> Result<(), String> {
    let cur = repo_flow(pg, host_id, repo).await?;
    let base = base_branch.unwrap_or(cur.base_branch);
    sqlx::query(
        "insert into workbench_repo_flow (host_id, repo, base_branch) \
         values ($1, $2, $3) \
         on conflict (host_id, repo) do update set \
           base_branch = excluded.base_branch, updated_at = now()",
    )
    .bind(host_id)
    .bind(repo)
    .bind(base)
    .execute(pg)
    .await
    .map_err(|e| format!("repo flow write: {e}"))?;
    Ok(())
}

/// The branch jobs cut from and PRs target — the flow override, else the
/// repo's default branch on its host.
pub async fn effective_base(pg: &PgPool, host: &Host, repo: &str) -> Result<String, String> {
    match repo_flow(pg, host.id(), repo).await?.base_branch {
        Some(base) => Ok(base),
        None => host.default_branch(repo).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_branch_law_decides_every_shape() {
        let rule = |base: Option<&str>, mode: &str, prefix: Option<&str>| RepoRule {
            host_id: "github".into(),
            repo: "acme/widgets".into(),
            base_branch: base.map(String::from),
            push_mode: mode.into(),
            branch_prefix: prefix.map(String::from),
        };
        // The DEFAULT posture: any branch but the base.
        let d = rule(None, "branches_only", None);
        assert!(push_allowed(&d, "main", "refs/heads/agent/thing").is_ok());
        assert!(push_allowed(&d, "main", "refs/heads/main").is_err());
        assert!(push_allowed(&d, "main", "refs/tags/v1").is_ok()); // tags are not branch pushes
        // A named base is refused — and so is the repo's default, which the
        // named base used to leave open: a flow based on `release` still
        // has a `main` that promotion owns.
        let named = rule(Some("release"), "branches_only", None);
        assert!(push_allowed(&named, "main", "refs/heads/main").is_err());
        assert!(push_allowed(&named, "main", "refs/heads/release").is_err());
        assert!(push_allowed(&named, "main", "refs/heads/agent/thing").is_ok());
        // The well-known promotion branches are refused whatever the rule
        // names: a repo whose flow base is `rc` (this repo's own model)
        // keeps `main` and `master` closed too.
        let rc = rule(Some("rc"), "branches_only", None);
        for b in ["main", "master", "rc"] {
            assert!(
                push_allowed(&rc, "develop", &format!("refs/heads/{b}")).is_err(),
                "{b}"
            );
        }
        assert!(push_allowed(&rc, "develop", "refs/heads/develop").is_err());
        assert!(push_allowed(&rc, "develop", "refs/heads/main-ish").is_ok());
        assert!(push_allowed(&rc, "develop", "refs/heads/rc/agent-1").is_ok());
        // A prefix narrows the agent's own branches.
        let pfx = rule(None, "branches_only", Some("agent/"));
        assert!(push_allowed(&pfx, "main", "refs/heads/agent/ticket-1").is_ok());
        assert!(push_allowed(&pfx, "main", "refs/heads/feature-x").is_err());
        // 'free' is the explicit trust: everything goes, base included.
        let free = rule(None, "free", Some("agent/"));
        assert!(push_allowed(&free, "main", "refs/heads/main").is_ok());
        assert!(push_allowed(&free, "main", "refs/heads/master").is_ok());
    }
}
