// What a maintainer reads about its plugin: `plugin_status`.
//
// One answer, every part of it a read: the commit installed and the one its
// branch points at now, the repository's open pull requests with each one's
// checks and review decision (`talaria_github::pull_request_status`, the
// same read `pr_status` makes), whether the app is enabled and whether its
// build loads, and what the app host made of its harnesses. A part that
// cannot be read says so in its own `…Error` field rather than failing the
// whole answer: a GitHub outage should not hide that the build is broken.

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_apps::lifecycle::{Credentials, auth_for};
use talaria_apps::source::{local_sources_allowed, parse_source};
use talaria_secretbox::SecretBox;
use talaria_users::{app_build_status, app_builds_dir, broken_apps};

use crate::Plugin;

/// How many open PRs one status reads checks and reviews for. Each costs
/// four GETs; a plugin with more open PRs than this lists the rest without
/// their checks.
pub const STATUS_PR_LIMIT: usize = 10;

/// The harness report the app host's load probe wrote beside the build
/// (`harnesses: { registered, problems }` in current.json). None when the
/// app ships no harnesses or the pointer cannot be read.
pub fn harness_report(slug: &str) -> Option<Value> {
    std::fs::read_to_string(app_builds_dir().join(slug).join("current.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("harnesses").cloned())
        .filter(Value::is_object)
}

/// The app as the host sees it: enabled, its build, and the manifest refusal
/// when talaria.json fails the rules (an SDK range this host is outside of,
/// among others).
pub async fn app_health(pg: &PgPool, slug: &str) -> Value {
    let enabled = talaria_apps::enabled_app_slugs(pg)
        .await
        .iter()
        .any(|s| s == slug);
    let build = app_build_status(slug);
    let broken = broken_apps()
        .into_iter()
        .find(|b| b.slug == slug)
        .map(|b| b.reason);
    let installed = broken.is_some()
        || talaria_users::discovered_apps()
            .iter()
            .any(|a| a.slug == slug);
    json!({
        "installed": installed,
        "enabled": enabled,
        "build": {
            "status": build.status,
            "error": build.error,
        },
        "broken": broken,
    })
}

/// The commit `plugin`'s branch points at now, asked of the remote.
pub async fn latest_commit(
    creds: &impl Credentials,
    plugin: &Plugin,
) -> Result<Option<String>, String> {
    if plugin
        .branch
        .as_deref()
        .is_some_and(talaria_apps::source::is_commit)
    {
        return Ok(plugin.branch.clone());
    }
    let src = parse_source(&plugin.url, local_sources_allowed())?;
    let (auth, note) = auth_for(creds, &src, None).await?;
    talaria_apps::git::remote_head(&src, plugin.branch.as_deref(), &auth)
        .await
        .map_err(|e| match note {
            Some(n) => format!("{e} — {n}"),
            None => e,
        })
}

/// The whole `plugin_status` answer.
pub async fn plugin_status(
    pg: &PgPool,
    sb: &SecretBox,
    creds: &impl Credentials,
    plugin: &Plugin,
) -> Value {
    let mut out = json!({
        "slug": plugin.app_slug,
        "repo": plugin.repo,
        "url": plugin.url,
        "gitHost": plugin.git_host_id,
        "branch": plugin.branch,
        "installedCommit": plugin.installed_commit,
        "updatePolicy": plugin.update_policy,
        "maintainer": plugin.maintainer_model,
        "owner": plugin.owner_name,
        "boardId": plugin.board_id,
        "board": plugin.board_name,
        "ticket": plugin.ticket_ref,
    });
    match latest_commit(creds, plugin).await {
        Ok(latest) => {
            out["updateAvailable"] =
                json!(latest.is_some() && latest.as_deref() != plugin.installed_commit.as_deref());
            out["latestCommit"] = json!(latest);
        }
        Err(e) => {
            out["latestCommit"] = Value::Null;
            out["latestCommitError"] = json!(e);
        }
    }
    match plugin.git_host_id.as_deref() {
        Some("github") => {
            match talaria_github::open_pull_requests(pg, sb, &plugin.repo, 30).await {
                Ok(prs) => {
                    let mut wire = Vec::with_capacity(prs.len());
                    for (i, pr) in prs.iter().enumerate() {
                        let mut row = serde_json::to_value(pr).unwrap_or(Value::Null);
                        if i < STATUS_PR_LIMIT {
                            match talaria_github::pull_request_status(
                                pg,
                                sb,
                                &plugin.repo,
                                pr.number,
                            )
                            .await
                            {
                                Ok(status) => {
                                    row["checks"] = status["checks"].clone();
                                    row["reviewDecision"] = status["reviewDecision"].clone();
                                    row["headSha"] = status["headSha"].clone();
                                    if let Some(errors) = status.get("errors") {
                                        row["errors"] = errors.clone();
                                    }
                                }
                                Err(e) => row["statusError"] = json!(e),
                            }
                        }
                        wire.push(row);
                    }
                    out["openPullRequests"] = Value::Array(wire);
                }
                Err(e) => {
                    out["openPullRequests"] = json!([]);
                    out["pullRequestsError"] = json!(e);
                }
            }
        }
        _ => {
            out["openPullRequests"] = json!([]);
            out["pullRequestsError"] = json!(format!(
                "pull requests are read from GitHub only, and {} is not on GitHub",
                plugin.url
            ));
        }
    }
    out["app"] = app_health(pg, &plugin.app_slug).await;
    out["harnesses"] = harness_report(&plugin.app_slug).unwrap_or(Value::Null);
    out
}
