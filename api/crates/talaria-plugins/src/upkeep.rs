// The maintenance pass over every plugin — a READ of each plugin's repo, and
// writes only to Talaria's own tickets and approvals.
//
// It runs inside the workbench job sweep (`workbench-job-sweep`, every ten
// minutes), one plugin at a time, four questions each:
//
//   pull requests  each open PR on the plugin's repo: its checks and its
//                  review decision (`talaria_github::pull_request_status`,
//                  every call a GET). Failing checks or a change request is a
//                  comment on the PR's ticket and the maintainer is put back
//                  to work on it — once per head commit for checks, once per
//                  change request for reviews, never every ten minutes. A PR
//                  that left the open list is read once more to learn whether
//                  it merged.
//   updates        the commit the plugin's branch points at now. When it is
//                  not the one installed (a merged PR moved it), an
//                  `app_update` approval is raised for an admin; a plugin on
//                  `update_policy = 'auto'` is updated straight away through
//                  talaria-apps' `update_app` (fetch, build, probe, swap,
//                  roll back on failure). Either way the outcome is a line on
//                  the plugin's ticket.
//   health         the app's build and manifest. A failed build, a manifest
//                  this host refuses, or a build that broke when the host was
//                  upgraded is a ticket on the plugin's board for its
//                  maintainer — one open ticket per plugin per kind.
//   evals          the plugin's harness columns in Models → Fitness. A cell
//                  whose band got worse since the last pass is an
//                  eval-regression ticket, same rule.
//
// TALARIA STOPS AT THE PR. Nothing here merges, pushes, comments on GitHub or
// edits the repository: the only GitHub calls are GETs, and the only update
// that ever runs is an admin's approval or the admin's own `auto` policy.

use std::collections::{BTreeMap, HashSet};

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_apps::{AppHost, PlatformCredentials};
use talaria_audit::{AuditEntry, log_audit};
use talaria_secretbox::SecretBox;
use talaria_tasks::{
    NewTask, TaskActor, TaskDeps, TaskPatch, add_comment, create_task, dispatch_ticket_of,
    get_task, log_activity, update_task,
};

use crate::{Plugin, short_sha};

/// The name the sweep writes under: ticket lines, comments and the tickets
/// it files.
pub const UPKEEP_ACTOR: &str = "talaria";

/// Everything the pass reaches for.
#[derive(Clone)]
pub struct UpkeepDeps {
    pub pg: PgPool,
    pub sb: SecretBox,
    pub redis: Option<redis::aio::ConnectionManager>,
    pub host: AppHost,
}

impl UpkeepDeps {
    fn tasks(&self) -> TaskDeps {
        TaskDeps::from_route(self.pg.clone(), self.redis.clone())
    }
}

/// What one pass did, for the sweep's summary line.
#[derive(Debug, Default)]
pub struct UpkeepReport {
    pub plugins: usize,
    pub resumed: usize,
    pub merged: usize,
    pub updates_raised: usize,
    pub updates_applied: usize,
    pub notices: usize,
    pub errors: usize,
}

impl UpkeepReport {
    /// One sentence, or None when nothing happened.
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if self.resumed > 0 {
            parts.push(format!(
                "reported {} plugin PR(s) to their maintainer",
                self.resumed
            ));
        }
        if self.merged > 0 {
            parts.push(format!("saw {} plugin PR(s) merge", self.merged));
        }
        if self.updates_raised > 0 {
            parts.push(format!(
                "raised {} plugin update approval(s)",
                self.updates_raised
            ));
        }
        if self.updates_applied > 0 {
            parts.push(format!(
                "applied {} automatic plugin update(s)",
                self.updates_applied
            ));
        }
        if self.notices > 0 {
            parts.push(format!("filed {} plugin ticket(s)", self.notices));
        }
        if self.errors > 0 {
            parts.push(format!("{} plugin read(s) failed", self.errors));
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// The whole pass. Err only when the plugins themselves cannot be read; a
/// failure on one plugin is logged, counted and left for the next pass.
pub async fn upkeep_pass(deps: &UpkeepDeps) -> Result<UpkeepReport, String> {
    let plugins = crate::plugins(&deps.pg)
        .await
        .map_err(|e| format!("plugin scan: {e}"))?;
    let mut report = UpkeepReport {
        plugins: plugins.len(),
        ..Default::default()
    };
    for p in &plugins {
        if let Err(e) = pr_pass(deps, p, &mut report).await {
            report.errors += 1;
            tracing::warn!("[plugins] {}: pull request pass failed: {e}", p.app_slug);
        }
        if let Err(e) = update_pass(deps, p, &mut report).await {
            report.errors += 1;
            tracing::warn!("[plugins] {}: update pass failed: {e}", p.app_slug);
        }
        if let Err(e) = health_pass(deps, p, &mut report).await {
            report.errors += 1;
            tracing::warn!("[plugins] {}: health pass failed: {e}", p.app_slug);
        }
        if let Err(e) = eval_pass(deps, p, &mut report).await {
            report.errors += 1;
            tracing::warn!("[plugins] {}: eval pass failed: {e}", p.app_slug);
        }
    }
    Ok(report)
}

// ── Pull requests ────────────────────────────────────────────────────────────

/// What a PR's status asks of its maintainer that has not been asked yet:
/// the head commit whose checks fail (`checks`), and the latest change
/// request (`review`, its submission time). `seen_*` are the markers the
/// last report recorded; a marker equal to the one recorded is not news.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PrNews {
    pub checks: Option<String>,
    pub review: Option<String>,
}

/// Pure, so the once-only rules are pinned.
pub fn pr_news(status: &Value, seen_checks: Option<&str>, seen_review: Option<&str>) -> PrNews {
    let head = status["headSha"].as_str().unwrap_or_default();
    let checks = (status["checks"]["verdict"] == "failing" && !head.is_empty())
        .then(|| head.to_string())
        .filter(|h| seen_checks != Some(h.as_str()));
    // The latest change request, by when it was submitted: a change request
    // stays the decision across pushes until the reviewer reviews again, so
    // keying it on the head would wake the maintainer after every fix.
    let review = if status["reviewDecision"] == "changes_requested" {
        status["reviews"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["state"] == "changes_requested")
            .filter_map(|r| r["submittedAt"].as_str())
            .max()
            .map(String::from)
            .or_else(|| Some(format!("head:{head}")))
            .filter(|at| seen_review != Some(at.as_str()))
    } else {
        None
    };
    PrNews { checks, review }
}

/// The comment the PR's ticket gets. Pure, so its wording is pinned.
pub fn pr_comment(status: &Value, news: &PrNews, resumed: &str) -> String {
    let number = status["number"].as_i64().unwrap_or_default();
    let url = status["url"].as_str().unwrap_or_default();
    let title = status["title"].as_str().unwrap_or_default();
    let mut what = Vec::new();
    if news.checks.is_some() {
        let failing: Vec<String> = status["checks"]["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| {
                matches!(
                    r["conclusion"].as_str(),
                    Some(
                        "failure"
                            | "timed_out"
                            | "cancelled"
                            | "action_required"
                            | "startup_failure"
                    )
                )
            })
            .filter_map(|r| r["name"].as_str().map(String::from))
            .chain(
                status["checks"]["statuses"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|s| matches!(s["state"].as_str(), Some("failure" | "error")))
                    .filter_map(|s| s["context"].as_str().map(String::from)),
            )
            .collect();
        what.push(if failing.is_empty() {
            "its checks are failing".to_string()
        } else {
            format!("its checks are failing ({})", failing.join(", "))
        });
    }
    if news.review.is_some() {
        let by: Vec<String> = status["reviews"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|r| r["state"] == "changes_requested")
            .filter_map(|r| r["by"].as_str().map(String::from))
            .collect();
        what.push(if by.is_empty() {
            "a reviewer requested changes".to_string()
        } else {
            format!("{} requested changes", by.join(", "))
        });
    }
    format!(
        "Plugin PR #{number} ({title}) needs work: {}. {url}\n\n{resumed} Read it with `pr_status`, push the fix to the same branch, and `finish_job` keeps the PR. Talaria never merges it — a person does.",
        what.join(" and "),
    )
}

/// The workbench job that opened PR `number` on `repo`, if one did: its
/// ticket and its agent.
async fn job_for_pr(
    pg: &PgPool,
    repo: &str,
    number: i64,
) -> Result<Option<(String, Option<String>, String)>, sqlx::Error> {
    sqlx::query_as(
        "select id::text, task_id::text, agent_model from workbench_jobs \
         where lower(repo) = lower($1) \
           and (pr_number = $2 or pr_url like '%/pull/' || $2::text) \
         order by updated_at desc limit 1",
    )
    .bind(repo)
    .bind(number)
    .fetch_optional(pg)
    .await
}

/// Put `agent` back to work on ticket `task_id`: assign it if it is not, and
/// when the ticket was handed off for review, send it back to the board's
/// working column — the move a reviewer's "request changes" makes. A ticket
/// already in a working column is offered to dispatch, the same push a
/// ticket entering a pickup column gets (a live session stands down, a
/// person's stop mark holds). A ticket a person parked anywhere else —
/// intake, Blocked — is left where it is. Answers the sentence the comment
/// carries.
async fn resume(deps: &UpkeepDeps, task_id: &str, agent: &str, why: &str) -> String {
    let tdeps = deps.tasks();
    let Ok(Some(task)) = get_task(&deps.pg, task_id).await else {
        return "The ticket is gone, so no agent was resumed.".into();
    };
    let Ok(meta) = talaria_statuses::status_meta(&deps.pg, &task.board_id).await else {
        return "The board's columns could not be read, so no agent was resumed.".into();
    };
    if task.archived_at.is_some() || meta.terminal(&task.status) {
        return "The ticket is closed, so no agent was resumed: reopen it, or ask for a follow-up."
            .into();
    }
    let in_review = meta.review_keys.contains(&task.status);
    if !in_review && !meta.working_keys.contains(&task.status) {
        return format!(
            "The ticket sits in {}, so no agent was resumed: move it to a working column to put {agent} on it.",
            task.status
        );
    }
    let mut patch = TaskPatch::default();
    if !task.assignees.iter().any(|a| a == agent) {
        let mut assignees = task.assignees.clone();
        assignees.push(agent.to_string());
        patch.assignees = Some(assignees);
    }
    if in_review {
        match meta
            .active_key
            .clone()
            .or_else(|| meta.assigned_key.clone())
        {
            Some(target) => {
                patch.status = Some(target);
                patch.status_note = Some(why.to_string());
            }
            None => {
                return "The board has no working column to send the ticket back to, so no agent was resumed."
                    .into();
            }
        }
    }
    if (patch.status.is_some() || patch.assignees.is_some())
        && let Err(e) =
            update_task(&tdeps, task_id, patch, &TaskActor::platform(UPKEEP_ACTOR)).await
    {
        return format!("{agent} could not be put back on it: {}.", e.message());
    }
    // Offered to dispatch whether or not the write above already pushed it
    // (update_task pushes on entering an agent-start column or gaining an
    // agent): the claim is a derived run id, so a second offer stands down.
    if let Ok(Some(fresh)) = get_task(&deps.pg, task_id).await
        && meta.pickup_keys.contains(&fresh.status)
    {
        talaria_tasks_types::maybe_dispatch_ticket(
            &deps.pg,
            &tdeps.dispatch,
            &dispatch_ticket_of(&fresh),
            Some(&[agent.to_string()]),
        )
        .await;
    }
    format!("Talaria put {agent} back on this ticket to fix it.")
}

async fn pr_pass(deps: &UpkeepDeps, p: &Plugin, report: &mut UpkeepReport) -> Result<(), String> {
    if p.git_host_id.as_deref() != Some("github") {
        return Ok(());
    }
    let pg = &deps.pg;
    let open = talaria_github::open_pull_requests(pg, &deps.sb, &p.repo, 30).await?;
    let open_numbers: HashSet<i64> = open.iter().map(|pr| pr.number).collect();

    // PRs that were open last pass and are not now: merged or closed.
    let watched: Vec<(i64, Option<String>)> = sqlx::query_as(
        "select pr_number, task_id::text from plugin_pr_watch where app_slug = $1 and state = 'open'",
    )
    .bind(&p.app_slug)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("watch read: {e}"))?;
    for (number, task_id) in watched {
        if open_numbers.contains(&number) {
            continue;
        }
        let status = talaria_github::pull_request_status(pg, &deps.sb, &p.repo, number).await?;
        let state = status["state"].as_str().unwrap_or("closed").to_string();
        sqlx::query(
            "update plugin_pr_watch set state = $3, seen_at = now() \
             where app_slug = $1 and pr_number = $2",
        )
        .bind(&p.app_slug)
        .bind(number)
        .bind(&state)
        .execute(pg)
        .await
        .map_err(|e| format!("watch write: {e}"))?;
        if state == "merged" {
            report.merged += 1;
            if let Some(task_id) = task_id.as_deref().or(p.task_id.as_deref()) {
                let line = format!(
                    "plugin PR #{number} merged into {} — {}",
                    status["base"].as_str().unwrap_or("its base"),
                    status["url"].as_str().unwrap_or_default()
                );
                let _ = log_activity(pg, task_id, UPKEEP_ACTOR, "workbench", &line).await;
            }
        }
    }

    for pr in open.iter().take(crate::status::STATUS_PR_LIMIT) {
        let status = talaria_github::pull_request_status(pg, &deps.sb, &p.repo, pr.number).await?;
        let job = job_for_pr(pg, &p.repo, pr.number)
            .await
            .map_err(|e| format!("job read: {e}"))?;
        let task_id = job
            .as_ref()
            .and_then(|(_, t, _)| t.clone())
            .or_else(|| p.task_id.clone());
        let seen: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "select checks_signalled, review_signalled from plugin_pr_watch \
             where app_slug = $1 and pr_number = $2",
        )
        .bind(&p.app_slug)
        .bind(pr.number)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("watch read: {e}"))?;
        let (seen_checks, seen_review) = seen.unwrap_or_default();
        let news = pr_news(&status, seen_checks.as_deref(), seen_review.as_deref());
        let anything = news.checks.is_some() || news.review.is_some();
        // Reported when there is a ticket to say it on and an agent to tell;
        // the markers are recorded either way, so the same news is never
        // reported twice. The maintainer is put back to work only on a PR a
        // workbench job opened — its own branch. A PR somebody else opened
        // is said on the plugin's ticket and left to its author.
        if anything
            && let (Some(task_id), Some(agent)) =
                (task_id.as_deref(), p.maintainer_model.as_deref())
        {
            let why = if news.checks.is_some() {
                format!("CI failing on plugin PR #{}", pr.number)
            } else {
                format!("changes requested on plugin PR #{}", pr.number)
            };
            let resumed = if job.is_some() {
                resume(deps, task_id, agent, &why).await
            } else {
                format!(
                    "No workbench job opened it (its author is {}), so no agent was put on it.",
                    if pr.author.is_empty() {
                        "unknown"
                    } else {
                        pr.author.as_str()
                    }
                )
            };
            let comment = pr_comment(&status, &news, &resumed);
            if let Err(e) = add_comment(&deps.tasks(), task_id, UPKEEP_ACTOR, &comment, None).await
            {
                tracing::warn!(
                    "[plugins] {}: PR comment on {task_id} failed: {e}",
                    p.app_slug
                );
            }
            report.resumed += 1;
        }
        sqlx::query(
            "insert into plugin_pr_watch (app_slug, pr_number, head_sha, state, checks, review, \
                checks_signalled, review_signalled, task_id, seen_at) \
             values ($1, $2, $3, 'open', $4, $5, $6, $7, $8::uuid, now()) \
             on conflict (app_slug, pr_number) do update set head_sha = excluded.head_sha, \
                state = 'open', checks = excluded.checks, review = excluded.review, \
                checks_signalled = coalesce(excluded.checks_signalled, plugin_pr_watch.checks_signalled), \
                review_signalled = coalesce(excluded.review_signalled, plugin_pr_watch.review_signalled), \
                task_id = excluded.task_id, seen_at = now()",
        )
        .bind(&p.app_slug)
        .bind(pr.number)
        .bind(status["headSha"].as_str())
        .bind(status["checks"]["verdict"].as_str())
        .bind(status["reviewDecision"].as_str())
        .bind(news.checks.as_deref())
        .bind(news.review.as_deref())
        .bind(task_id.as_deref())
        .execute(pg)
        .await
        .map_err(|e| format!("watch write: {e}"))?;
    }
    Ok(())
}

// ── Updates ──────────────────────────────────────────────────────────────────

/// The line an update's outcome writes on the plugin's ticket.
pub fn update_line(slug: &str, outcome: &talaria_apps::Updated, by: &str) -> String {
    let to = short_sha(outcome.to.as_deref());
    let from = short_sha(outcome.from.as_deref());
    if outcome.up_to_date {
        format!("{by}: plugin \"{slug}\" is already running {to}")
    } else if outcome.ok {
        format!("{by}: updated plugin \"{slug}\" from {from} to {to}")
    } else if outcome.rolled_back {
        format!(
            "{by}: the update of plugin \"{slug}\" to {to} failed and was rolled back — it still runs {from}: {}",
            outcome.error.as_deref().unwrap_or("unknown error")
        )
    } else {
        format!(
            "{by}: the update of plugin \"{slug}\" to {to} failed and could not be rolled back — apps/{slug} holds {to}, which did not load: {}",
            outcome.error.as_deref().unwrap_or("unknown error")
        )
    }
}

/// Run `update_app` for a plugin and record its outcome: the audit row, and
/// the line on the plugin's ticket. Answers the line.
pub async fn apply_update(
    pg: &PgPool,
    creds: &PlatformCredentials<'_>,
    host: &AppHost,
    p: &Plugin,
    actor: &str,
    by: &str,
) -> Result<(talaria_apps::Updated, String), String> {
    let outcome = talaria_apps::update_app(pg, creds, host, &p.app_slug, None, actor).await?;
    let line = update_line(&p.app_slug, &outcome, by);
    if !outcome.up_to_date {
        log_audit(
            pg,
            AuditEntry {
                actor,
                action: if outcome.ok {
                    "app.update"
                } else {
                    "app.update-failed"
                },
                target_type: "app",
                target_id: Some(&p.app_slug),
                target_label: outcome.error.as_deref(),
                before: Some(json!({ "sha": outcome.from })),
                after: Some(json!({
                    "sha": if outcome.ok || !outcome.rolled_back { outcome.to.clone() } else { outcome.from.clone() },
                    "ref": outcome.git_ref,
                    "attempted": outcome.to,
                    "rolledBack": outcome.rolled_back,
                    "plugin": true,
                })),
            },
        )
        .await;
    }
    if let Some(task_id) = p.task_id.as_deref()
        && let Err(e) = log_activity(pg, task_id, actor, "workbench", &line).await
    {
        tracing::warn!(
            "[plugins] {}: update line on {task_id} failed: {e}",
            p.app_slug
        );
    }
    Ok((outcome, line))
}

/// The merged PRs not yet named on an update, newest first.
async fn merged_since(pg: &PgPool, slug: &str) -> Result<Vec<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "update plugin_pr_watch set update_named = true \
         where app_slug = $1 and state = 'merged' and not update_named \
         returning pr_number",
    )
    .bind(slug)
    .fetch_all(pg)
    .await
}

async fn update_pass(
    deps: &UpkeepDeps,
    p: &Plugin,
    report: &mut UpkeepReport,
) -> Result<(), String> {
    let pg = &deps.pg;
    if p.branch
        .as_deref()
        .is_some_and(talaria_apps::source::is_commit)
    {
        return Ok(());
    }
    let creds = PlatformCredentials {
        pg,
        sb: &deps.sb,
        actor: UPKEEP_ACTOR,
    };
    let Some(latest) = crate::status::latest_commit(&creds, p).await? else {
        return Ok(());
    };
    if p.installed_commit.as_deref() == Some(latest.as_str()) {
        // Running the newest commit: a request still waiting for an older
        // one has nothing left to do.
        sqlx::query(
            "update app_update_requests set status = 'superseded', \
                result = 'the plugin already runs ' || $2, updated_at = now() \
             where app_slug = $1 and status = 'pending'",
        )
        .bind(&p.app_slug)
        .bind(short_sha(Some(&latest)))
        .execute(pg)
        .await
        .map_err(|e| format!("request write: {e}"))?;
        return Ok(());
    }
    let prs = merged_since(pg, &p.app_slug)
        .await
        .map_err(|e| format!("watch read: {e}"))?;
    let pending: Option<(String, String)> = sqlx::query_as(
        "select id::text, target_sha from app_update_requests \
         where app_slug = $1 and status in ('pending', 'updating') limit 1",
    )
    .bind(&p.app_slug)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("request read: {e}"))?;
    if let Some((id, target)) = pending {
        // One waiting update per plugin, following the branch: approving it
        // installs whatever the branch points at then.
        if target != latest {
            sqlx::query(
                "update app_update_requests set target_sha = $2, \
                    prs = prs || $3, updated_at = now() where id = $1::uuid and status = 'pending'",
            )
            .bind(&id)
            .bind(&latest)
            .bind(json!(prs))
            .execute(pg)
            .await
            .map_err(|e| format!("request write: {e}"))?;
        }
        return Ok(());
    }
    // A commit an admin already turned down, or one that already failed to
    // apply, is not asked about again: the next merge moves the branch.
    let decided: Option<String> = sqlx::query_scalar(
        "select status from app_update_requests where app_slug = $1 and target_sha = $2 \
         and status in ('rejected', 'failed') limit 1",
    )
    .bind(&p.app_slug)
    .bind(&latest)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("request read: {e}"))?;
    if decided.is_some() {
        return Ok(());
    }
    let prs_words = if prs.is_empty() {
        String::new()
    } else {
        format!(
            " (merged: {})",
            prs.iter()
                .map(|n| format!("#{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if p.update_policy == "auto" {
        let (outcome, line) =
            apply_update(pg, &creds, &deps.host, p, UPKEEP_ACTOR, "automatic update").await?;
        let status = if outcome.ok { "applied" } else { "failed" };
        sqlx::query(
            "insert into app_update_requests (app_slug, target_sha, git_ref, prs, status, \
                decided_by, result) values ($1, $2, $3, $4, $5, 'auto', $6)",
        )
        .bind(&p.app_slug)
        .bind(outcome.to.as_deref().unwrap_or(&latest))
        .bind(p.branch.as_deref())
        .bind(json!(prs))
        .bind(status)
        .bind(&line)
        .execute(pg)
        .await
        .map_err(|e| format!("request write: {e}"))?;
        report.updates_applied += 1;
        return Ok(());
    }
    let id: String = sqlx::query_scalar(
        "insert into app_update_requests (app_slug, target_sha, git_ref, prs) \
         values ($1, $2, $3, $4) returning id::text",
    )
    .bind(&p.app_slug)
    .bind(&latest)
    .bind(p.branch.as_deref())
    .bind(json!(prs))
    .fetch_one(pg)
    .await
    .map_err(|e| format!("request write: {e}"))?;
    // Told now, through the one announcer: the approval sweep would find it
    // within five minutes, and a merged fix should not wait for that.
    announce(deps, format!("app_update:{id}")).await;
    if let Some(task_id) = p.task_id.as_deref() {
        let line = format!(
            "plugin \"{}\": {} is on {}{prs_words} — the update from {} waits for an admin's approval in Manage → Apps",
            p.app_slug,
            short_sha(Some(&latest)),
            p.branch.as_deref().unwrap_or("its default branch"),
            short_sha(p.installed_commit.as_deref()),
        );
        let _ = log_activity(pg, task_id, UPKEEP_ACTOR, "workbench", &line).await;
    }
    tracing::info!("[plugins] {}: update approval {id} raised", p.app_slug);
    report.updates_raised += 1;
    Ok(())
}

/// Announce a raised approval through talaria-approvals' one announcer, which
/// decides who may be told and how much (`announce_approval`). Idempotent
/// against the approval sweep: both mark the same key.
async fn announce(deps: &UpkeepDeps, key: String) {
    let approvals = talaria_approvals::ApprovalDeps::new(
        deps.pg.clone(),
        talaria_realtime::RealtimeDeps::publish_only(deps.redis.clone()),
        std::sync::Arc::new(talaria_runs_define::run_definition),
        std::sync::Arc::new(talaria_agent_auth::now_ms),
    );
    talaria_approvals::announce_approval(&approvals, &key).await;
}

// ── Notices: tickets on the plugin's board ───────────────────────────────────

/// The kinds of ticket the pass files. One open ticket per plugin per kind.
pub const NOTICE_KINDS: [&str; 3] = ["build_failed", "host_upgrade", "eval_regression"];

/// File a ticket on the plugin's board for its maintainer, unless one of
/// this kind is still open. Answers whether a ticket was filed.
pub async fn open_notice(
    deps: &UpkeepDeps,
    p: &Plugin,
    kind: &str,
    title: &str,
    body: &str,
) -> Result<bool, String> {
    let pg = &deps.pg;
    let Some(board_id) = p.board_id.as_deref() else {
        return Ok(false);
    };
    let open: Vec<(String, String, String, bool)> = sqlx::query_as(
        "select t.id::text, t.board_id::text, t.status, t.archived_at is not null \
         from plugin_notices n join tasks t on t.id = n.task_id \
         where n.app_slug = $1 and n.kind = $2",
    )
    .bind(&p.app_slug)
    .bind(kind)
    .fetch_all(pg)
    .await
    .map_err(|e| format!("notice read: {e}"))?;
    for (_, board, status, archived) in &open {
        if *archived {
            continue;
        }
        let terminal = talaria_statuses::status_meta(pg, board)
            .await
            .map(|m| m.terminal(status))
            .unwrap_or(false);
        if !terminal {
            return Ok(false);
        }
    }
    let assignees: Vec<String> = p.maintainer_model.iter().cloned().collect();
    let description = format!(
        "{body}\n\n— Filed by Talaria's plugin upkeep for **{}** ({}{}). Its maintainer is {}. Read it with `plugin_status`; Talaria never merges or pushes — open a PR with the fix.",
        p.app_slug,
        p.repo,
        p.branch
            .as_deref()
            .map(|b| format!(" @ {b}"))
            .unwrap_or_default(),
        p.maintainer_model.as_deref().unwrap_or("nobody"),
    );
    let task = create_task(
        &deps.tasks(),
        &NewTask {
            board_id,
            title,
            description: Some(&description),
            priority: Some("high"),
            effort: None,
            assignees: &assignees,
            due_date: None,
            start_date: None,
            color: None,
            estimated_hours: None,
            parent_id: None,
            tags: &[],
            created_by: UPKEEP_ACTOR,
        },
    )
    .await
    .map_err(|e| format!("ticket create: {}", e.message()))?;
    sqlx::query(
        "insert into plugin_notices (app_slug, kind, task_id, detail) values ($1, $2, $3::uuid, $4)",
    )
    .bind(&p.app_slug)
    .bind(kind)
    .bind(&task.id)
    .bind(body)
    .execute(pg)
    .await
    .map_err(|e| format!("notice write: {e}"))?;
    Ok(true)
}

/// The app's build pointer as the host wrote it: status, error, and the host
/// build it was made on.
fn build_pointer(slug: &str) -> (String, Option<String>, Option<String>) {
    let raw = std::fs::read_to_string(
        talaria_users::app_builds_dir()
            .join(slug)
            .join("current.json"),
    )
    .ok()
    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    .unwrap_or(Value::Null);
    (
        raw["status"].as_str().unwrap_or("none").to_string(),
        raw["error"].as_str().map(String::from),
        raw["hostBuild"].as_str().map(String::from),
    )
}

/// What the app's health asks of its maintainer. Pure, so the rules are
/// pinned: a manifest this host refuses for its SDK range is a host-upgrade
/// break (the refusal names the range the app needs); any other refusal and
/// a failed build are a build failure, unless the app last built on another
/// host build, which makes it a break after the host was upgraded.
pub fn health_notice(
    status: &str,
    error: Option<&str>,
    host_build: Option<&str>,
    broken: Option<&str>,
    last_ok_host: Option<&str>,
) -> Option<(&'static str, String)> {
    if let Some(reason) = broken {
        if reason.contains("needs @talaria/sdk") {
            return Some((
                "host_upgrade",
                format!(
                    "This host refuses the app's talaria.json: {reason}. Widen the app's `sdk` range to the SDK this host provides, and fix what that version changed."
                ),
            ));
        }
        return Some((
            "build_failed",
            format!("This host refuses the app's talaria.json: {reason}."),
        ));
    }
    if status != "failed" {
        return None;
    }
    let error = error.unwrap_or("the app host gave no reason");
    match (last_ok_host, host_build) {
        (Some(ok), Some(now)) if ok != now => Some((
            "host_upgrade",
            format!(
                "The app built on host build {ok} and fails since the host was upgraded to {now}: {error}"
            ),
        )),
        _ => Some(("build_failed", format!("The app's build fails: {error}"))),
    }
}

fn notice_title(kind: &str, slug: &str) -> String {
    match kind {
        "host_upgrade" => format!("[plugin {slug}] broke after the host upgrade"),
        "eval_regression" => format!("[plugin {slug}] harness evals regressed"),
        _ => format!("[plugin {slug}] build is failing"),
    }
}

async fn upkeep_state(pg: &PgPool, slug: &str) -> Result<Value, String> {
    sqlx::query_scalar::<_, Value>("select upkeep from plugins where app_slug = $1")
        .bind(slug)
        .fetch_optional(pg)
        .await
        .map(|v| v.unwrap_or_else(|| json!({})))
        .map_err(|e| format!("plugin read: {e}"))
}

async fn set_upkeep(pg: &PgPool, slug: &str, key: &str, value: Value) -> Result<(), String> {
    sqlx::query(
        "update plugins set upkeep = jsonb_set(upkeep, array[$2], $3, true) where app_slug = $1",
    )
    .bind(slug)
    .bind(key)
    .bind(value)
    .execute(pg)
    .await
    .map(|_| ())
    .map_err(|e| format!("plugin write: {e}"))
}

async fn health_pass(
    deps: &UpkeepDeps,
    p: &Plugin,
    report: &mut UpkeepReport,
) -> Result<(), String> {
    let (status, error, host_build) = build_pointer(&p.app_slug);
    let broken = talaria_users::broken_apps()
        .into_iter()
        .find(|b| b.slug == p.app_slug)
        .map(|b| b.reason);
    let state = upkeep_state(&deps.pg, &p.app_slug).await?;
    let last_ok_host = state["okHost"].as_str().map(String::from);
    if status == "ready"
        && let Some(host) = &host_build
        && last_ok_host.as_deref() != Some(host.as_str())
    {
        set_upkeep(&deps.pg, &p.app_slug, "okHost", json!(host)).await?;
    }
    if let Some((kind, body)) = health_notice(
        &status,
        error.as_deref(),
        host_build.as_deref(),
        broken.as_deref(),
        last_ok_host.as_deref(),
    ) && open_notice(deps, p, kind, &notice_title(kind, &p.app_slug), &body).await?
    {
        report.notices += 1;
    }
    Ok(())
}

// ── Evals ────────────────────────────────────────────────────────────────────

/// A band's rank, best first; None for a cell that is not a verdict
/// (untested, unbound).
fn band_rank(band: &str) -> Option<u8> {
    match band {
        "ready" => Some(0),
        "workable" => Some(1),
        "unfit" => Some(2),
        _ => None,
    }
}

/// The plugin's harness columns in the fitness matrix: model → column →
/// (band, reason). The matrix index is `model_fitness_index`; an app
/// harness's column key is `app:<slug>/<harness id>`.
pub fn app_columns(
    index: &Value,
    slug: &str,
) -> BTreeMap<String, BTreeMap<String, (String, Option<String>)>> {
    let prefix = format!("app:{slug}/");
    let mut out = BTreeMap::new();
    for (model, entry) in index.as_object().into_iter().flatten() {
        let mut cols = BTreeMap::new();
        for (key, cell) in entry["cells"].as_object().into_iter().flatten() {
            if let Some(harness) = key.strip_prefix(&prefix) {
                cols.insert(
                    harness.to_string(),
                    (
                        cell["band"].as_str().unwrap_or("untested").to_string(),
                        cell["reason"].as_str().map(String::from),
                    ),
                );
            }
        }
        if !cols.is_empty() {
            out.insert(model.clone(), cols);
        }
    }
    out
}

/// The cells whose band got worse since `seen` (model → column → band).
/// Pure, so what counts as a regression is pinned.
pub fn regressions(
    seen: &Value,
    now: &BTreeMap<String, BTreeMap<String, (String, Option<String>)>>,
) -> Vec<String> {
    let mut out = Vec::new();
    for (model, cols) in now {
        for (harness, (band, reason)) in cols {
            let Some(before) = seen[model][harness].as_str() else {
                continue;
            };
            if let (Some(b), Some(n)) = (band_rank(before), band_rank(band))
                && n > b
            {
                out.push(format!(
                    "{harness} on {model}: {before} → {band}{}",
                    reason
                        .as_deref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default()
                ));
            }
        }
    }
    out
}

async fn eval_pass(deps: &UpkeepDeps, p: &Plugin, report: &mut UpkeepReport) -> Result<(), String> {
    let index =
        talaria_gateway::settings::get_setting(&deps.pg, "model_fitness_index", json!({})).await;
    let now = app_columns(&index, &p.app_slug);
    if now.is_empty() {
        return Ok(());
    }
    let state = upkeep_state(&deps.pg, &p.app_slug).await?;
    let found = regressions(&state["bands"], &now);
    let bands: serde_json::Map<String, Value> = now
        .iter()
        .map(|(model, cols)| {
            (
                model.clone(),
                Value::Object(
                    cols.iter()
                        .map(|(h, (band, _))| (h.clone(), json!(band)))
                        .collect(),
                ),
            )
        })
        .collect();
    set_upkeep(&deps.pg, &p.app_slug, "bands", Value::Object(bands)).await?;
    if found.is_empty() {
        return Ok(());
    }
    let body = format!(
        "The app's harness evals got worse in Models → Fitness since the last check:\n\n{}\n\nRead the columns with `app_harness_fitness`.",
        found
            .iter()
            .map(|l| format!("- {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    if open_notice(
        deps,
        p,
        "eval_regression",
        &notice_title("eval_regression", &p.app_slug),
        &body,
    )
    .await?
    {
        report.notices += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(verdict: &str, decision: &str, reviews: Value) -> Value {
        json!({
            "number": 7,
            "url": "https://github.com/acme/notes/pull/7",
            "title": "Add tags",
            "headSha": "beef",
            "checks": {
                "verdict": verdict,
                "runs": [
                    { "name": "test", "status": "completed", "conclusion": "failure" },
                    { "name": "lint", "status": "completed", "conclusion": "success" },
                ],
                "statuses": [{ "context": "ci/build", "state": "error" }],
            },
            "reviewDecision": decision,
            "reviews": reviews,
        })
    }

    #[test]
    fn failing_checks_are_news_once_per_head_commit() {
        let s = status("failing", "none", json!([]));
        assert_eq!(
            pr_news(&s, None, None),
            PrNews {
                checks: Some("beef".into()),
                review: None
            }
        );
        assert_eq!(pr_news(&s, Some("beef"), None), PrNews::default());
        // A new head that fails again is news again.
        assert_eq!(
            pr_news(&s, Some("cafe"), None).checks.as_deref(),
            Some("beef")
        );
        // Passing or pending checks are never news.
        assert_eq!(
            pr_news(&status("pending", "none", json!([])), None, None),
            PrNews::default()
        );
    }

    #[test]
    fn a_change_request_is_news_once_whatever_the_maintainer_pushes_after_it() {
        let reviews = json!([
            { "by": "ann", "state": "changes_requested", "submittedAt": "2026-10-26T10:00:00Z" },
            { "by": "bo", "state": "approved", "submittedAt": "2026-10-26T11:00:00Z" },
        ]);
        let s = status("passing", "changes_requested", reviews.clone());
        let news = pr_news(&s, None, None);
        assert_eq!(news.review.as_deref(), Some("2026-10-26T10:00:00Z"));
        assert_eq!(news.checks, None);
        // Same change request, a new head pushed since: not news.
        let mut pushed = s.clone();
        pushed["headSha"] = json!("cafe");
        assert_eq!(
            pr_news(&pushed, None, Some("2026-10-26T10:00:00Z")).review,
            None
        );
        // A later change request is.
        let later = status(
            "passing",
            "changes_requested",
            json!([{ "by": "ann", "state": "changes_requested", "submittedAt": "2026-10-27T09:00:00Z" }]),
        );
        assert_eq!(
            pr_news(&later, None, Some("2026-10-26T10:00:00Z"))
                .review
                .as_deref(),
            Some("2026-10-27T09:00:00Z")
        );
    }

    #[test]
    fn the_comment_names_what_failed_and_who_asked_and_never_offers_a_merge() {
        let s = status(
            "failing",
            "changes_requested",
            json!([{ "by": "ann", "state": "changes_requested", "submittedAt": "t" }]),
        );
        let news = pr_news(&s, None, None);
        let c = pr_comment(
            &s,
            &news,
            "Talaria put dev-eng back on this ticket to fix it.",
        );
        assert!(c.starts_with("Plugin PR #7 (Add tags) needs work: its checks are failing (test, ci/build) and ann requested changes."), "{c}");
        assert!(c.contains("Talaria put dev-eng back on this ticket"));
        assert!(c.contains("Talaria never merges it"));
        assert!(!c.contains("lint"), "a passing check is not named");
    }

    #[test]
    fn health_reads_an_sdk_refusal_as_a_host_upgrade_break_and_names_the_range() {
        let (kind, body) = health_notice(
            "none",
            None,
            None,
            Some("this app needs @talaria/sdk ^2.0; this host provides 1.0.0"),
            None,
        )
        .unwrap();
        assert_eq!(kind, "host_upgrade");
        assert!(body.contains("^2.0") && body.contains("1.0.0"), "{body}");
        let (kind, _) =
            health_notice("none", None, None, Some("\"name\" is required"), None).unwrap();
        assert_eq!(kind, "build_failed");
        // A failure on the host build it last built on is the app's own.
        let (kind, body) =
            health_notice("failed", Some("app.ts: x"), Some("h1"), None, Some("h1")).unwrap();
        assert_eq!(kind, "build_failed");
        assert!(body.ends_with("app.ts: x"));
        // One on a new host build is the upgrade's.
        let (kind, body) =
            health_notice("failed", Some("app.ts: x"), Some("h2"), None, Some("h1")).unwrap();
        assert_eq!(kind, "host_upgrade");
        assert!(body.contains("h1") && body.contains("h2"), "{body}");
        assert_eq!(
            health_notice("ready", None, Some("h2"), None, Some("h1")),
            None
        );
    }

    #[test]
    fn a_regression_is_a_band_that_got_worse_and_nothing_else() {
        let index = json!({
            "model-a": { "at": "t", "cells": {
                "app:notes/notes:triage": { "band": "unfit", "reason": "contract held 2/10" },
                "app:notes/notes:tag": { "band": "ready" },
                "app:other/x": { "band": "unfit" },
                "role:utility": { "band": "unfit" },
            }},
            "model-b": { "at": "t", "cells": { "role:utility": { "band": "ready" } } },
        });
        let now = app_columns(&index, "notes");
        assert_eq!(now.len(), 1, "only models with this app's columns");
        assert_eq!(now["model-a"].len(), 2);
        let seen = json!({ "model-a": { "notes:triage": "ready", "notes:tag": "workable" } });
        assert_eq!(
            regressions(&seen, &now),
            ["notes:triage on model-a: ready → unfit (contract held 2/10)"]
        );
        // Nothing seen yet is no regression; untested is not a verdict.
        assert!(regressions(&json!({}), &now).is_empty());
        let untested = app_columns(
            &json!({ "m": { "cells": { "app:notes/notes:tag": { "band": "untested" } } } }),
            "notes",
        );
        assert!(regressions(&json!({ "m": { "notes:tag": "ready" } }), &untested).is_empty());
    }

    #[test]
    fn an_update_line_says_where_the_app_stands() {
        let mut u = talaria_apps::Updated {
            ok: true,
            slug: "notes".into(),
            from: Some("1111111aaaa".into()),
            to: Some("2222222bbbb".into()),
            ..Default::default()
        };
        assert_eq!(
            update_line("notes", &u, "approved update"),
            "approved update: updated plugin \"notes\" from 1111111 to 2222222"
        );
        u.ok = false;
        u.rolled_back = true;
        u.error = Some("probe failed".into());
        assert_eq!(
            update_line("notes", &u, "automatic update"),
            "automatic update: the update of plugin \"notes\" to 2222222 failed and was rolled back — it still runs 1111111: probe failed"
        );
    }

    #[test]
    fn the_summary_says_what_the_pass_did_or_nothing() {
        assert_eq!(UpkeepReport::default().summary(), None);
        let r = UpkeepReport {
            resumed: 1,
            updates_raised: 1,
            notices: 2,
            ..Default::default()
        };
        assert_eq!(
            r.summary().as_deref(),
            Some(
                "reported 1 plugin PR(s) to their maintainer, raised 1 plugin update approval(s), filed 2 plugin ticket(s)"
            )
        );
    }
}
