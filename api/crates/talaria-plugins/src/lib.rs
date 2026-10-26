// Plugins — installed apps that a Developer Agent builds and keeps
// maintaining, for a person, from a repo it was granted.
//
// WHAT A PLUGIN IS. An app is code under apps/<slug>/; where that code came
// from (repo, ref, the commit running) is its `app_sources` row, owned by
// talaria-apps. A PLUGIN is an app that also has people and an agent behind
// it: the agent that maintains it, the user it was built for, the board its
// maintenance tickets go to (the board of the ticket that started it), and
// the admin's update policy. That is the `plugins` row, and nothing on it
// repeats the source: repo, branch and installed commit are read from
// `app_sources` through the join, so the two can never disagree.
//
//   install   an agent asks (`request_app_install`, `install::file_request`);
//             an admin approves in Manage → Apps; the approval runs
//             talaria-apps' credentialed install and records the plugin.
//             Approval INSTALLS and nothing more: enabling the app and
//             granting its surfaces stay admin steps.
//   status    `status::plugin_status` — everything a maintainer reads about
//             its plugin, every call a read.
//
// TALARIA STOPS AT THE PR. Nothing in this crate merges, pushes or writes to
// a plugin's repository.

pub mod install;
pub mod status;

#[cfg(test)]
mod live;

use serde::Serialize;
use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;

/// The admin's choice of how a plugin's new versions land: every update
/// waits for an admin's approval (`approve`, the default), or it is applied
/// as soon as it is seen (`auto`).
pub const UPDATE_POLICIES: [&str; 2] = ["approve", "auto"];

/// The git host a source host is, as the git-host registry will name it.
/// Only GitHub has a connection today; any other host is unnamed (None).
pub fn git_host_of(host: &str) -> Option<&'static str> {
    (host == "github.com").then_some("github")
}

/// One plugin, with its source read through `app_sources`.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Plugin {
    pub app_slug: String,
    pub git_host_id: Option<String>,
    /// `owner/name` on GitHub.
    pub repo: String,
    pub url: String,
    pub host: String,
    /// The ref the app follows; None is the repository's default branch.
    pub branch: Option<String>,
    /// The commit running now (`app_sources.sha`).
    pub installed_commit: Option<String>,
    pub maintainer_agent_id: Option<String>,
    pub maintainer_model: Option<String>,
    pub maintainer_name: Option<String>,
    pub owner_user_id: Option<String>,
    pub owner_name: Option<String>,
    pub board_id: Option<String>,
    pub board_name: Option<String>,
    /// The ticket that started the plugin.
    pub task_id: Option<String>,
    pub ticket_ref: Option<String>,
    pub update_policy: String,
    pub created_at: String,
}

#[derive(sqlx::FromRow)]
struct PluginRow {
    app_slug: String,
    git_host_id: Option<String>,
    repo: String,
    url: String,
    host: String,
    git_ref: Option<String>,
    sha: Option<String>,
    maintainer_agent_id: Option<String>,
    maintainer_model: Option<String>,
    maintainer_name: Option<String>,
    owner_user_id: Option<String>,
    owner_name: Option<String>,
    board_id: Option<String>,
    board_name: Option<String>,
    task_id: Option<String>,
    ticket_ref: Option<String>,
    update_policy: String,
    created_ms: i64,
}

const PLUGIN_SELECT: &str = "select p.app_slug, p.git_host_id, s.repo, s.url, s.host, s.git_ref, s.sha, \
        p.maintainer_agent_id::text as maintainer_agent_id, d.model as maintainer_model, \
        d.display_name as maintainer_name, p.owner_user_id::text as owner_user_id, \
        coalesce(nullif(u.name, ''), u.email) as owner_name, \
        p.board_id::text as board_id, b.name as board_name, p.task_id::text as task_id, \
        case when t.ticket_no is not null then coalesce(tb.ticket_prefix, 'TASK') || '-' || t.ticket_no end as ticket_ref, \
        p.update_policy, (trunc(extract(epoch from p.created_at) * 1000))::bigint as created_ms \
     from plugins p \
     join app_sources s on s.slug = p.app_slug \
     left join agent_defs d on d.id = p.maintainer_agent_id \
     left join users u on u.id = p.owner_user_id \
     left join boards b on b.id = p.board_id \
     left join tasks t on t.id = p.task_id \
     left join boards tb on tb.id = t.board_id";

fn plugin_of(r: PluginRow) -> Plugin {
    Plugin {
        app_slug: r.app_slug,
        git_host_id: r.git_host_id,
        repo: r.repo,
        url: r.url,
        host: r.host,
        branch: r.git_ref,
        installed_commit: r.sha,
        maintainer_agent_id: r.maintainer_agent_id,
        maintainer_model: r.maintainer_model,
        maintainer_name: r.maintainer_name,
        owner_user_id: r.owner_user_id,
        owner_name: r.owner_name,
        board_id: r.board_id,
        board_name: r.board_name,
        task_id: r.task_id,
        ticket_ref: r.ticket_ref,
        update_policy: r.update_policy,
        created_at: epoch_ms_to_iso(r.created_ms),
    }
}

/// Every plugin, by slug.
pub async fn plugins(pg: &PgPool) -> Result<Vec<Plugin>, sqlx::Error> {
    let rows: Vec<PluginRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PLUGIN_SELECT} order by p.app_slug"
    )))
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(plugin_of).collect())
}

/// One plugin, or None when `slug` is not one.
pub async fn plugin(pg: &PgPool, slug: &str) -> Result<Option<Plugin>, sqlx::Error> {
    let row: Option<PluginRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PLUGIN_SELECT} where p.app_slug = $1"
    )))
    .bind(slug)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(plugin_of))
}

/// The plugins an agent maintains.
pub async fn plugins_maintained_by(
    pg: &PgPool,
    agent_id: &str,
) -> Result<Vec<Plugin>, sqlx::Error> {
    let rows: Vec<PluginRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{PLUGIN_SELECT} where p.maintainer_agent_id = $1::uuid order by p.app_slug"
    )))
    .bind(agent_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(plugin_of).collect())
}

/// What recording a plugin writes.
pub struct NewPlugin<'a> {
    pub slug: &'a str,
    pub git_host_id: Option<&'a str>,
    pub maintainer_agent_id: Option<&'a str>,
    pub owner_user_id: Option<&'a str>,
    pub board_id: Option<&'a str>,
    pub task_id: Option<&'a str>,
    pub created_by: &'a str,
}

/// Record (or re-record) an installed app as a plugin. The app must have its
/// `app_sources` row (a git install); a reinstall keeps the admin's update
/// policy and takes the new maintainer, owner and board.
pub async fn record_plugin(pg: &PgPool, p: &NewPlugin<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into plugins (app_slug, git_host_id, maintainer_agent_id, owner_user_id, \
            board_id, task_id, created_by) \
         values ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6::uuid, $7) \
         on conflict (app_slug) do update set git_host_id = excluded.git_host_id, \
            maintainer_agent_id = excluded.maintainer_agent_id, \
            owner_user_id = excluded.owner_user_id, board_id = excluded.board_id, \
            task_id = excluded.task_id, updated_at = now()",
    )
    .bind(p.slug)
    .bind(p.git_host_id)
    .bind(p.maintainer_agent_id)
    .bind(p.owner_user_id)
    .bind(p.board_id)
    .bind(p.task_id)
    .bind(p.created_by)
    .execute(pg)
    .await?;
    Ok(())
}

/// Set a plugin's update policy. Ok(None) when `slug` is not a plugin;
/// Ok(Some(previous)) otherwise.
pub async fn set_update_policy(
    pg: &PgPool,
    slug: &str,
    policy: &str,
) -> Result<Option<String>, String> {
    if !UPDATE_POLICIES.contains(&policy) {
        return Err(format!(
            "updatePolicy must be one of {} — got \"{policy}\"",
            UPDATE_POLICIES.join(", ")
        ));
    }
    let previous: Option<(String,)> =
        sqlx::query_as("select update_policy from plugins where app_slug = $1")
            .bind(slug)
            .fetch_optional(pg)
            .await
            .map_err(|e| format!("plugin read: {e}"))?;
    let Some((previous,)) = previous else {
        return Ok(None);
    };
    sqlx::query("update plugins set update_policy = $2, updated_at = now() where app_slug = $1")
        .bind(slug)
        .bind(policy)
        .execute(pg)
        .await
        .map_err(|e| format!("plugin write: {e}"))?;
    Ok(Some(previous))
}

/// The first seven characters of a commit, or "—".
pub fn short_sha(sha: Option<&str>) -> String {
    sha.map(|s| s.chars().take(7).collect())
        .unwrap_or_else(|| "—".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_github_is_a_named_git_host_today() {
        assert_eq!(git_host_of("github.com"), Some("github"));
        assert_eq!(git_host_of("gitlab.example.com"), None);
        assert_eq!(git_host_of(""), None);
    }

    #[test]
    fn a_short_sha_is_seven_characters_or_a_dash() {
        assert_eq!(short_sha(Some("0123456789abcdef")), "0123456");
        assert_eq!(short_sha(None), "—");
    }
}
