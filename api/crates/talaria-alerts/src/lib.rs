// Derived system alerts — computed from live state on every read, no tables.
// Sources: managed-container reality (docker), gateway reachability, the token
// ledger's blind spots (unpriced cloud usage, estimated counts), the background
// scheduler (failing jobs, and whether it is running at all), the notification
// outbox (depth, age, breaker, what has been lost), what each GitHub App
// installation has accepted (the record talaria-git-hosts keeps — never GitHub
// itself), the agents given GitHub permissions it has not, a git token revoke
// GitHub did not confirm, a granted repo an agent's git got no token for, and
// stuck tickets on boards the requesting user can access.
//
// THE RULE THIS FILE IS FOR: anything whose failure mode is SILENCE has to be
// asked here, because nothing else asks it. A dead container is obvious the
// moment you use the product; a paused mail queue, a job that stopped running
// and a scheduler that never started are all indistinguishable from a quiet
// week. Every one of those has a live number somewhere in the process — the
// work is bringing it to a person, not computing it.
//
// The scheduler and outbox rows describe the
// ANSWERING process's own memory — on a multi-instance deployment
// that is whichever instance answered.

use std::collections::HashMap;

use serde::Serialize;

use talaria_agent_auth::now_ms;
use talaria_boards::board_visibility_sql;
use talaria_brain_health::fleet_brain_health;
use talaria_fleet_agents::list_agents;
use talaria_fleet_docker::container_status;
use talaria_fleet_preflight::last_fleet_preflight;
use talaria_gateway::budget::group;
use talaria_gateway::usage::cost_overview;
use talaria_git_hosts::holdings::{HostHoldings, InstallationRow, STALE_AFTER_MS};
use talaria_mcp_service::mcp_port;
use talaria_notify::notification_mail_stats;
use talaria_retrieval::backfill::rag_health;
use talaria_retrieval::embed;
use talaria_retrieval::migrate::retrieval_upgrade_status;
use talaria_retrieval::qdrant;
use talaria_scheduler::{HealthSeverity, scheduler_status, unhealthy_jobs};
use talaria_state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertSeverity {
    Critical,
    Warning,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    pub severity: AlertSeverity,
    pub title: String,
    pub detail: String,
    pub href: &'static str,
}

/// How long a mail may sit in the outbox before the queue is "stuck" rather
/// than "busy". The drain runs every 30 seconds and gives itself a 25-second
/// budget, so ten drains' worth of waiting is not a backlog moving slowly — it
/// is a backlog not moving.
const OUTBOX_STALE_MS: i64 = 5 * 60_000;

/// Consecutive tool failures before the tool-plane alert fires. Ten is a
/// full minute of a thrashing agent (the plugin reports one frame per call)
/// and an ordinary bad tool or two never reaches it.
pub const TOOL_FAIL_STREAK_ALERT: i64 = 10;

// Polled by /alerts (60s) and Home (30s): a short cache keeps repeat loads
// instant, and the probes below run in PARALLEL — serially they added up to
// seconds (docker exec + four network probes).
static ALERTS_CACHE: std::sync::LazyLock<tokio::sync::Mutex<HashMap<String, (i64, Vec<Alert>)>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));
const ALERTS_TTL_MS: i64 = 15_000;

pub async fn compute_alerts(state: &AppState, user_id: &str) -> Vec<Alert> {
    {
        let cache = ALERTS_CACHE.lock().await;
        if let Some((at, value)) = cache.get(user_id)
            && now_ms() - at < ALERTS_TTL_MS
        {
            return value.clone();
        }
    }
    let value = compute_alerts_fresh(state, user_id).await;
    ALERTS_CACHE
        .lock()
        .await
        .insert(user_id.to_string(), (now_ms(), value.clone()));
    value
}

/// The alert for a department two or more agents hold.
fn shared_department_alert(share: &talaria_agent_defs::SharedDepartment) -> Alert {
    let d = &share.department;
    let detail = match share.runner() {
        Some(runner) => {
            let others: Vec<&str> = share
                .members
                .iter()
                .filter(|m| m.id != runner.id)
                .map(|m| m.display_name.as_str())
                .collect();
            // The render's rule (the older agent runs) is not the one the
            // render kept before it (the slug that sorted last): on an
            // install upgraded with the pair in place, the container can
            // still hold the other agent until something recreates it. A
            // deploy's fleet roll does; on a checkout, the admin's Roll does.
            format!(
                "{runner} runs in its container (agent-{d}) and keeps its memory; {} {} left out of the fleet. Retire and delete one of them — Talaria never merges two agents into one. If agent-{d} still runs {} (an install upgraded with the pair in place), Roll {runner} to put it back in its container.",
                others.join(", "),
                if others.len() == 1 { "is" } else { "are" },
                others.join(" or "),
                runner = runner.display_name,
            )
        }
        None => format!(
            "None of them is running. A department is one container (agent-{d}) and one memory volume: retire and delete all but one before starting it."
        ),
    };
    Alert {
        severity: AlertSeverity::Warning,
        title: format!(
            "{} agents share the department \"{d}\"",
            share.members.len()
        ),
        detail,
        href: "/agents",
    }
}

/// en-US grouping (1,234,567) via the crate's shared grouping helper.
fn fmt(n: i64) -> String {
    group(n)
}

/// The alert sentences' minute count — rounded half-up, then clamped to >= 1
/// by every caller. A hair under 90s still reads as "2"; a hair under 30s
/// reads as "1", never 0.
fn minutes(ms: i64) -> i64 {
    ((ms as f64) / 60_000.0).round() as i64
}

async fn compute_alerts_fresh(state: &AppState, user_id: &str) -> Vec<Alert> {
    let pg = &state.pg;
    let mut alerts: Vec<Alert> = Vec::new();

    // ── Fleet: every enabled managed agent should have a running container ─────
    let managed: Vec<(String, String, String, String)> = sqlx::query_as(
        "select slug, department, display_name, model from agent_defs \
         where managed and enabled order by slug",
    )
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    // Everything independent, at once — the wall-clock is max(probe), not sum.
    let departments: Vec<String> = managed.iter().map(|m| m.1.clone()).collect();
    let states_fut = async {
        if managed.is_empty() {
            None
        } else {
            container_status(&departments).await.ok()
        }
    };
    let manifest_count_fut = async { list_agents().await.len() };
    let preflight_fut = last_fleet_preflight(pg);
    let mcp_up_fut = probe_mcp();
    let rag_fut = async {
        let qd = qdrant::real_deps();
        let ed = embed::real_deps();
        rag_health(&qd, &ed).await
    };
    let upgrade_fut = async {
        let qd = qdrant::real_deps();
        let ed = embed::real_deps();
        retrieval_upgrade_status(pg, &qd, &ed).await.ok()
    };
    let brains_fut = fleet_brain_health(pg);
    // Per-agent consecutive tool-failure streaks, maintained by the
    // tool-events route (`agent-tools:<model>:fail-streak`). None-value per
    // agent = no streak key = tools healthy or silent; the alert below says
    // which of those it is not.
    let streak_keys: Vec<String> = managed
        .iter()
        .map(|m| format!("agent-tools:{}:fail-streak", m.3))
        .collect();
    let toolplane_fut = async {
        if streak_keys.is_empty() {
            Vec::new()
        } else if let Ok(mut conn) = state.redis().await {
            redis::cmd("MGET")
                .arg(&streak_keys)
                .query_async::<Vec<Option<i64>>>(&mut conn)
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    };
    let cost_fut = cost_overview(pg);
    let vis = board_visibility_sql("$1", "$2", false);
    let stuck_sql = format!(
        "select t.id::text, t.title, t.status, b.id::text as board_id, b.name as board, \
                extract(day from now() - t.updated_at)::int as days \
         from tasks t \
         join boards b on b.id = t.board_id \
         where {vis} \
           and t.archived_at is null \
           and (t.status = 'failed' or (t.status = 'blocked' and t.updated_at < now() - interval '7 days')) \
         order by t.updated_at asc limit 20"
    );
    let stuck_fut = sqlx::query_as::<_, (String, String, String, String, String, i32)>(
        sqlx::AssertSqlSafe(stuck_sql.as_str()),
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_all(pg);
    let (states, manifest_count, preflight, mcp_up, rag, upgrade, brains, cost, stuck, toolplane) = tokio::join!(
        states_fut,
        manifest_count_fut,
        preflight_fut,
        mcp_up_fut,
        rag_fut,
        upgrade_fut,
        brains_fut,
        cost_fut,
        stuck_fut,
        toolplane_fut
    );
    let stuck = stuck.unwrap_or_default();
    let cost = cost.ok();

    if !managed.is_empty()
        && let Some(states) = states.as_ref()
    {
        for (_, department, display_name, _) in &managed {
            let st = states
                .iter()
                .find(|s| &s.department == department)
                .and_then(|s| s.managed.as_ref());
            match st {
                None => alerts.push(Alert {
                    severity: AlertSeverity::Critical,
                    title: format!("{display_name} is down"),
                    detail: "no managed container exists. Render + up from /agents.".into(),
                    href: "/agents",
                }),
                Some(st) if st.state != "running" => alerts.push(Alert {
                    severity: AlertSeverity::Critical,
                    title: format!("{display_name} is down"),
                    detail: format!("container {} is {} ({})", st.name, st.state, st.status),
                    href: "/agents",
                }),
                Some(st) if st.status.to_lowercase().contains("unhealthy") => alerts.push(Alert {
                    severity: AlertSeverity::Warning,
                    title: format!("{display_name} is unhealthy"),
                    detail: st.status.clone(),
                    href: "/agents",
                }),
                _ => {}
            }
        }
    }

    // ── Fleet: one agent per department ─────────────────────────────────────────
    // A department is one container and one memory volume, so two agents in
    // one are one agent in practice. New hires cannot make such a pair; one
    // that predates the rule is said here, every read, until somebody unties
    // it — the render quietly running the older one is exactly the kind of
    // silence this file exists for.
    if let Ok(shared) = talaria_agent_defs::shared_departments(pg).await {
        alerts.extend(shared.iter().map(shared_department_alert));
    }

    // ── Gateway plane reachability ──────────────────────────────────────────────
    // Only a failure when agents EXIST but nothing is rendered. An empty manifest
    // on an instance with no managed agents is a fresh install, not an outage.
    if !managed.is_empty() && manifest_count == 0 {
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            title: "Gateway plane unreachable".into(),
            detail: "No rendered agent manifest. Talaria has no agent url or key to reach, so chat and channel replies will fail. Render your agents from /agents.".into(),
            href: "/agents",
        });
    }

    // ── Can an agent actually REACH us? ───────────────────────────────────────
    // Null means the probe has never run, which is not a failure and is not
    // reported as one.
    if let Some(p) = preflight.as_ref()
        && !p.ok
    {
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            title: "Agents cannot reach Talaria".into(),
            detail: p.detail.clone(),
            href: "/agents",
        });
    }

    // ── Agent toolkit endpoint: the fleet's MCP must answer or agents flail ────
    if !mcp_up {
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            title: "Agent toolkit (MCP) unreachable".into(),
            detail: format!(
                "The agent toolkit endpoint on :{} is not answering, so every agent's Talaria tools (knowledge, tickets, documents, research) are failing. It respawns on the next comms read; if this persists, check the app logs.",
                mcp_port()
            ),
            href: "/agents",
        });
    }

    // ── External MCP servers the health pass found down (admins only) ────────
    // The hourly `mcp-health` job (talaria-mcp health.rs) writes every
    // external server's `last_probe`; a server that is not `ok` is a set of
    // tools every agent carrying it calls into an error. Admins fix servers,
    // so only admins see the line — one alert naming them all, not one each.
    if is_admin(pg, user_id).await
        && let Some(alert) = mcp_attention_alert(&mcp_servers_needing_attention(pg).await)
    {
        alerts.push(alert);
    }

    // ── GitHub: what an installation has not accepted, or is unknown ─────────
    // Admins only, like the MCP line: an org owner approves on GitHub, and an
    // admin is who knows to ask one. Read from the holdings record the
    // refresh keeps (talaria-git-hosts), never from GitHub: this runs on
    // every page load. A permission change nobody has accepted is a token
    // minted without it, and an installation nobody could read is one whose
    // pending change shows nowhere — both are silence until someone looks.
    // Beside them, from the same record and the agents' permissions: an
    // agent given a GitHub permission the installation has not accepted
    // carries it on no token, which nothing else says; and a revoke GitHub
    // did not confirm in the last hour is a token still alive that an admin
    // meant to take back.
    if is_admin(pg, user_id).await {
        match talaria_git_hosts::holdings::recorded_holdings(pg).await {
            Ok(hosts) => {
                alerts.extend(git_host_alerts(&hosts, now_ms()));
                match talaria_git_hosts::permissions::agent_sets(pg).await {
                    Ok(sets) => alerts.extend(dormant_permissions_alert(
                        &talaria_git_hosts::permissions::dormant(&hosts, &sets),
                    )),
                    Err(e) => tracing::warn!("[alerts] {e}"),
                }
            }
            Err(e) => tracing::warn!("[alerts] {e}"),
        }
        match unconfirmed_revokes(pg).await {
            Ok(n) => alerts.extend(revoke_failed_alert(n)),
            Err(e) => tracing::warn!("[alerts] token revoke read failed: {e}"),
        }
        // A granted repo the git host gave an agent no token for: the push
        // fails in a sandbox nobody is watching, and the agent's git was told
        // why, which no admin otherwise hears.
        match refused_credentials(pg).await {
            Ok(rows) => alerts.extend(refused_credentials_alert(&rows)),
            Err(e) => tracing::warn!("[alerts] refused git credential read failed: {e}"),
        }
    }

    // ── Tool plane: a failing tool streak the green dashboards never show ─────
    // This is the 2026-09-22 rot detector: a persona whose every tool call
    // fails sits "healthy" on every state-only surface while its sessions
    // burn turns on retries. Warning, not Critical: the agent is alive and
    // a roll fixes it; the sentence says exactly that.
    for (streak, (_, _, display_name, _)) in toolplane.iter().zip(&managed) {
        let Some(streak) = streak else { continue };
        if *streak >= TOOL_FAIL_STREAK_ALERT {
            alerts.push(Alert {
                severity: AlertSeverity::Warning,
                title: format!("{display_name}'s tools are failing"),
                detail: format!(
                    "{streak} consecutive tool calls have failed. The agent keeps answering while \
                     every tool errors — sessions on it will thrash. Roll the agent from /agents \
                     (or stop its work) and check its container logs for the sqlite/fd cascade.",
                ),
                href: "/agents",
            });
        }
    }
    // ── Retrieval plane: Qdrant + embeddings must be up or the brains starve ────
    if !rag.qdrant || !rag.embeddings {
        let down = [
            (!rag.qdrant).then_some("Qdrant (vector store)"),
            (!rag.embeddings).then_some("embeddings (TEI)"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" and ");
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            title: "Retrieval plane down".into(),
            detail: format!("{down} unreachable, so nothing new is being indexed and agent knowledge search fails. Start the services (docker/dev-compose.yml), then run the backfill in Admin → Retrieval."),
            href: "/admin",
        });
    } else if upgrade.as_ref().is_some_and(|u| u.dim_mismatch) {
        // Services up, but do the collections still match the live embedding
        // model? A TALARIA_EMBED_MODEL swap changes dimensions and every index/
        // search against the old collections fails just as silently.
        let u = upgrade.as_ref().expect("checked dim_mismatch above");
        let bad: Vec<&str> = u
            .collections
            .iter()
            .filter(|c| c.dim_mismatch)
            .map(|c| c.name.as_str())
            .collect();
        let is_one = bad.len() == 1;
        let (was, it) = if is_one {
            ("is", "it")
        } else {
            ("are", "them")
        };
        let model = u
            .embed
            .as_ref()
            .map(|e| e.model_id.clone())
            .unwrap_or_default();
        let dim = u.embed.as_ref().map(|e| e.dim).unwrap_or(0);
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            title: "Embedding model changed: brains need a rebuild".into(),
            detail: format!(
                "The embedding service now serves {model} ({dim}d) but {} {was} built at a different dimension, so indexing and search against {it} are failing. Run the rebuild in Admin → Retrieval.",
                bad.join(", ")
            ),
            href: "/admin",
        });
    }

    // ── Brain routability: configured models still on the registry? ────────────
    // Main = critical; tiers/fallbacks = one warning.
    for b in brains.unwrap_or_default() {
        let bad_main = b.targets.iter().find(|t| t.kind == "main" && !t.ok);
        if !b.ok {
            alerts.push(Alert {
                severity: AlertSeverity::Critical,
                title: format!("{}'s brain is unroutable", b.display_name),
                detail: match bad_main {
                    Some(t) => format!(
                        "{}/{}: {}. Chats will hang. Fix the model on /models or repoint the agent.",
                        t.endpoint, t.model, t.reason.unwrap_or("")
                    ),
                    None => "no main model configured, so the agent has nothing to think with".into(),
                },
                href: "/agents",
            });
        } else {
            let degraded: Vec<_> = b.targets.iter().filter(|t| !t.ok).collect();
            if !degraded.is_empty() {
                let one = degraded.len() == 1;
                alerts.push(Alert {
                    severity: AlertSeverity::Warning,
                    title: format!(
                        "{}: {} {} unroutable",
                        b.display_name,
                        degraded.len(),
                        if one {
                            "tier/fallback is"
                        } else {
                            "tiers/fallbacks are"
                        }
                    ),
                    detail: degraded
                        .iter()
                        .map(|t| {
                            format!(
                                "{}{} → {}/{} ({})",
                                t.kind,
                                t.name
                                    .as_deref()
                                    .map(|n| format!(" \"{n}\""))
                                    .unwrap_or_default(),
                                t.endpoint,
                                t.model,
                                t.reason.unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; "),
                    href: "/agents",
                });
            }
        }
    }

    // ── Background jobs ─────────────────────────────────────────────────────────
    for job in unhealthy_jobs(now_ms()) {
        alerts.push(Alert {
            severity: match job.severity {
                HealthSeverity::Critical => AlertSeverity::Critical,
                HealthSeverity::Warning => AlertSeverity::Warning,
            },
            title: format!(
                "Background job \"{}\" is not running cleanly",
                job.name.as_str()
            ),
            detail: format!(
                "{} Reported by the instance that served this request.",
                job.detail
            ),
            href: "/observability",
        });
    }

    // ── Is the scheduler running AT ALL? ────────────────────────────────────────
    let jobs = scheduler_status(now_ms());
    let unarmed = jobs.iter().filter(|j| j.first_run_due_at.is_none()).count();
    let in_production = std::env::var("NODE_ENV").as_deref() == Ok("production");
    if jobs.is_empty() {
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            title: "No background jobs are registered on this instance".into(),
            detail: "The scheduler has an empty registry, so the daily digest, the approval SLA, comms decay and the notification mail drain are all not running, and none of them will report a failure, because none of them exists. Their modules were never imported by this build.".into(),
            href: "/observability",
        });
    } else if unarmed > 0 && unarmed < jobs.len() {
        // SOME armed and some not — a job whose module joined the registry
        // after `start_scheduler()` ran. Nothing will ever time it.
        let names: Vec<String> = jobs
            .iter()
            .filter(|j| j.first_run_due_at.is_none())
            .map(|j| format!("\"{}\"", j.name.as_str()))
            .collect();
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            title: format!(
                "Background job{} registered too late to be armed",
                if unarmed == 1 { "" } else { "s" }
            ),
            detail: format!(
                "{} registered after the scheduler started, so {} timed by nothing and will never run. A job's module has to be in the runtime graph before start_scheduler(): imported from the server entry, not lazily from a route.",
                names.join(", "),
                if unarmed == 1 { "it is" } else { "they are" }
            ),
            href: "/observability",
        });
    } else if unarmed == jobs.len() && std::env::var("TALARIA_SCHEDULER").as_deref() == Ok("off") {
        // The kill switch, on purpose: an operator set TALARIA_SCHEDULER=off
        // — a misbehaving job, an incident — so an empty arm table is the
        // state they asked for. Not an emergency, but worth the page saying
        // so: a switch left on is how a deployment quietly loses its digest.
        alerts.push(Alert {
            severity: AlertSeverity::Info,
            title: "Background jobs are deliberately off".into(),
            detail: "TALARIA_SCHEDULER=off, so this instance arms nothing by design — no digest, no approval escalation, no comms decay, no notification mail, anywhere until the switch comes off.".into(),
            href: "/observability",
        });
    } else if unarmed == jobs.len() {
        // Registered but never armed — and with this process the only
        // scheduler a deployment has, no other runtime is coming to do it.
        alerts.push(Alert {
            severity: if in_production { AlertSeverity::Critical } else { AlertSeverity::Info },
            title: if in_production {
                "Background jobs are not running on this instance"
            } else {
                "Background jobs are not armed (dev)"
            }
            .into(),
            detail: format!(
                "{} job(s) are registered and not one of them has been armed, so nothing is timing them: no digest, no approval escalation, no comms decay, no notification mail.{}",
                jobs.len(),
                if in_production {
                    " The arm loop never completed: Postgres or Redis never answered its probes, so arming is still retrying (see the startup log)."
                } else {
                    " Usually the arm loop still waiting on Postgres or Redis; the startup log says which."
                }
            ),
            href: "/observability",
        });
    }

    // ── The notification outbox ────────────────────────────────────────────────
    let mail = notification_mail_stats();
    if mail.paused_for_ms > 0 {
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            title: "Notification email is paused after repeated failures".into(),
            detail: format!(
                "Enough sends failed in a row that the outbox breaker opened, so nothing is being attempted for another {} minute(s). {} Check the mail provider (Admin → Org → Email).",
                minutes(mail.paused_for_ms).max(1),
                if mail.queued == 0 {
                    "Nothing is queued this second, but nothing queued before it closes will be attempted either.".to_string()
                } else {
                    format!(
                        "{} mail(s) are queued{}. Every one of them is still in its recipient’s in-app inbox. Only the email is late.",
                        mail.queued,
                        match mail.oldest_queued_ms {
                            None => String::new(),
                            Some(ms) => format!(", the oldest waiting {} minute(s)", minutes(ms).max(1)),
                        }
                    )
                }
            ),
            href: "/observability",
        });
    } else if mail.queued > 0 && mail.oldest_queued_ms.unwrap_or(0) > OUTBOX_STALE_MS {
        alerts.push(Alert {
            severity: AlertSeverity::Warning,
            title: "Notification email is backing up".into(),
            detail: format!(
                "{} of a possible {} mail(s) are queued and the oldest has been waiting {} minute(s), on a drain that runs every 30 seconds. The transport is not keeping up. Every one of them is still in its recipient’s in-app inbox.",
                mail.queued,
                mail.capacity,
                minutes(mail.oldest_queued_ms.unwrap_or(0)).max(1)
            ),
            href: "/observability",
        });
    } else if mail.queued >= mail.capacity / 2 {
        alerts.push(Alert {
            severity: AlertSeverity::Warning,
            title: "Notification email queue is filling up".into(),
            detail: format!(
                "{} of a possible {} mail(s) are queued. Past {} new mail is refused at the door: the notification still lands in the app, the email does not.",
                mail.queued, mail.capacity, mail.capacity
            ),
            href: "/observability",
        });
    }
    if mail.dropped > 0 || mail.abandoned > 0 {
        // Since boot, and cumulative on purpose: "it recovered" does not
        // un-lose them.
        let lost = [
            (mail.dropped > 0).then(|| {
                format!(
                    "{} refused at the door because the queue was full",
                    fmt(mail.dropped as i64)
                )
            }),
            (mail.abandoned > 0).then(|| {
                format!(
                    "{} given up on after repeated send failures",
                    fmt(mail.abandoned as i64)
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        alerts.push(Alert {
            severity: AlertSeverity::Warning,
            title: "Notification emails have been lost since boot".into(),
            detail: format!("{lost}. Each one is still unread in its recipient’s in-app inbox: the email was lost, the notification was not."),
            href: "/observability",
        });
    }

    // ── Ledger blind spots ──────────────────────────────────────────────────────
    if let Some(cost) = cost.as_ref() {
        // Admins only: the fix (a price, "included in a plan", a retried
        // lookup) is an admin's to make, on an admin-only panel — the line
        // used to send everyone to /models, where no price can be set.
        if let Some(alert) = unpriced_alert(cost.totals.unpriced_cloud_tokens)
            && is_admin(pg, user_id).await
        {
            alerts.push(alert);
        }
        let share = cost.totals.estimated_share.as_f64().unwrap_or(0.0);
        if cost.totals.month.generations >= 5 && share > 0.5 {
            alerts.push(Alert {
                severity: AlertSeverity::Warning,
                title: "Token counts are mostly estimates".into(),
                detail: format!(
                    "{}% of the last 30 days' generations lack real usage from the gateway.",
                    (share * 100.0).round() as i64
                ),
                href: "/observability/cost",
            });
        }
    }

    // ── Stuck work (scoped to boards this user can see) ────────────────────────
    for (id, title, status, board_id, board, days) in &stuck {
        if status == "failed" {
            alerts.push(Alert {
                severity: AlertSeverity::Warning,
                title: format!("Failed: {title}"),
                detail: format!("on {board}: needs a human decision"),
                href: leak(format!("/boards/{board_id}/{id}")),
            });
        } else {
            alerts.push(Alert {
                severity: AlertSeverity::Info,
                title: format!("Blocked {days}d: {title}"),
                detail: format!("on {board}: blocked for {days} days"),
                href: leak(format!("/boards/{board_id}/{id}")),
            });
        }
    }

    // A full disk or a box out of RAM is silence until someone looks. The
    // host read is the same snapshot Observability shows; a container that
    // cannot see the host contributes nothing here rather than a lie.
    push_host_pressure(&mut alerts).await;

    // Severity-rank sort — stable, severity-first.
    alerts.sort_by_key(|a| match a.severity {
        AlertSeverity::Critical => 0,
        AlertSeverity::Warning => 1,
        AlertSeverity::Info => 2,
    });
    alerts
}

/// Stuck-ticket hrefs are per-row paths, not literals. `sort_by_key` borrows
/// the vec while comparing, so each Alert keeps its own leaked &'static str —
/// at most twenty per read, freed never.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

async fn push_host_pressure(alerts: &mut Vec<Alert>) {
    let snap = talaria_host_metrics::snapshot().await;
    if !snap.available {
        return;
    }
    let hot: Vec<_> = snap.mounts.iter().filter(|m| m.percent >= 85.0).collect();
    if !hot.is_empty() {
        let critical = hot.iter().any(|m| m.percent >= 95.0);
        let title = if hot.len() == 1 {
            format!("{} is {:.0}% full", hot[0].mount, hot[0].percent)
        } else {
            format!("{} mounts are filling", hot.len())
        };
        let detail = hot
            .iter()
            .map(|m| format!("{} {:.0}%", m.mount, m.percent))
            .collect::<Vec<_>>()
            .join(", ");
        alerts.push(Alert {
            severity: if critical {
                AlertSeverity::Critical
            } else {
                AlertSeverity::Warning
            },
            title,
            detail,
            href: "/observability/host",
        });
    }
    if let Some(mem) = &snap.memory
        && mem.percent >= 90.0
    {
        alerts.push(Alert {
            severity: if mem.percent >= 97.0 {
                AlertSeverity::Critical
            } else {
                AlertSeverity::Warning
            },
            title: format!("Host memory is {:.0}% used", mem.percent),
            detail: format!(
                "{} available of {}",
                human_bytes(mem.available),
                human_bytes(mem.total)
            ),
            href: "/observability/host",
        });
    }
    if let Some(swap) = &snap.swap
        && swap.present
        && swap.percent >= 60.0
    {
        alerts.push(Alert {
            severity: AlertSeverity::Warning,
            title: format!("Host is swapping ({:.0}%)", swap.percent),
            detail: format!(
                "{} of {} swap in use. The box is out of RAM.",
                human_bytes(swap.used),
                human_bytes(swap.total)
            ),
            href: "/observability/host",
        });
    }
}

fn human_bytes(n: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if n >= 1 << 30 {
        format!("{:.1} GB", n as f64 / GIB)
    } else if n >= 1 << 20 {
        format!("{:.0} MB", n as f64 / MIB)
    } else {
        format!("{n} B")
    }
}

/// The one admin warning for every external MCP server whose last probe is
/// not ok — names them all, so a registry with three dead hosts is one line.
fn mcp_attention_alert(labels: &[String]) -> Option<Alert> {
    let n = labels.len();
    if n == 0 {
        return None;
    }
    let one = n == 1;
    Some(Alert {
        severity: AlertSeverity::Warning,
        title: format!(
            "{n} MCP server{} need{} attention",
            if one { "" } else { "s" },
            if one { "s" } else { "" }
        ),
        detail: format!(
            "{}. The last health check could not use {}, so agents calling {} tools get errors. Each card on the MCP page says what it needs.",
            labels.join(", "),
            if one { "it" } else { "them" },
            if one { "its" } else { "their" }
        ),
        href: "/mcp/external",
    })
}

/// Where a git host's alert sends an admin: Admin → Git hosts, whose host
/// card lists each installation's permissions and has the Refresh.
pub const GIT_HOSTS_HREF: &str = "/admin/git";

/// Where an alert about what agents' GitHub tokens carry sends an admin:
/// Agent policy → GitHub access, which lists every agent with its set and
/// its tokens' state, and whose editor names, per permission, what the
/// installation holds and where an owner accepts it on GitHub.
pub const GITHUB_ACCESS_HREF: &str = "/admin/github";

/// The GitHub App alerts, from the holdings record alone (`hosts`, as
/// `recorded_holdings` reads it): one warning naming every installation with
/// a permission change waiting for an owner of its account, and one naming
/// every installation whose permissions are unknown — never read, the last
/// read failed, or the last good read is older than the record trusts. A
/// cold or broken record is said, never read as "nothing pending".
pub fn git_host_alerts(hosts: &[HostHoldings], now_ms: i64) -> Vec<Alert> {
    let mut waiting: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for h in hosts {
        for id in &h.selected {
            let row = h.rows.iter().find(|r| &r.installation_id == id);
            let who = |r: Option<&InstallationRow>| match r
                .map(|r| r.account.as_str())
                .filter(|a| !a.is_empty())
            {
                Some(account) => format!("{account} (installation {id} on {})", h.host.name),
                None => format!("installation {id} on {}", h.host.name),
            };
            let Some(r) = row else {
                unknown.push(format!("{}: never checked", who(None)));
                continue;
            };
            match (&r.error, r.checked_at_ms) {
                (Some(e), _) => {
                    unknown.push(format!("{}: the last check failed ({e})", who(Some(r))))
                }
                (None, None) => unknown.push(format!("{}: never checked", who(Some(r)))),
                (None, Some(at)) if now_ms - at > STALE_AFTER_MS => unknown.push(format!(
                    "{}: not checked for {} minutes",
                    who(Some(r)),
                    minutes(now_ms - at)
                )),
                _ => {}
            }
            // The last good read's answer stands while a later one fails.
            let pending = r.pending();
            if !pending.is_empty() {
                waiting.push(format!(
                    "{} has not accepted {pending}{}",
                    who(Some(r)),
                    r.html_url
                        .as_deref()
                        .map(|u| format!(" (an owner accepts it at {u})"))
                        .unwrap_or_default()
                ));
            }
        }
    }
    let mut out = Vec::new();
    if !waiting.is_empty() {
        let one = waiting.len() == 1;
        out.push(Alert {
            severity: AlertSeverity::Warning,
            title: if one {
                "A GitHub App permission change is waiting for org-owner approval on GitHub".into()
            } else {
                format!(
                    "{} GitHub App permission changes are waiting for org-owner approval on GitHub",
                    waiting.len()
                )
            },
            detail: format!(
                "{}. The App asks for {}, and until an owner of the account accepts it on GitHub, Talaria mints tokens without it.",
                waiting.join("; "),
                if one { "it" } else { "them" }
            ),
            href: GIT_HOSTS_HREF,
        });
    }
    if !unknown.is_empty() {
        out.push(Alert {
            severity: AlertSeverity::Warning,
            title: format!(
                "GitHub permissions unknown for {} installation{}",
                unknown.len(),
                if unknown.len() == 1 { "" } else { "s" }
            ),
            detail: format!(
                "{}. Talaria cannot say what {} accepted, so a permission change waiting for an org owner would show nowhere. Press Refresh on the host in Admin → Git hosts; the record is read again every ten minutes.",
                unknown.join("; "),
                if unknown.len() == 1 { "it has" } else { "they have" }
            ),
            href: GIT_HOSTS_HREF,
        });
    }
    out
}

/// The alert for agents given a GitHub permission on a host whose
/// installation has not accepted it (as talaria-git-hosts' `dormant` finds
/// them): an admin set it in Talaria, and every token of those repos goes
/// without it until the GitHub App asks for it and an owner of the account
/// accepts the change on GitHub. Informational: nothing waits on it. None
/// when there are none.
pub fn dormant_permissions_alert(
    dormant: &[talaria_git_hosts::permissions::Dormant],
) -> Option<Alert> {
    if dormant.is_empty() {
        return None;
    }
    let mut agents: Vec<&str> = dormant.iter().map(|d| d.agent_id.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();
    let n = agents.len();
    let each: Vec<String> = dormant
        .iter()
        .map(|d| {
            format!(
                "{} on {}: {} ({})",
                d.agent_name,
                d.host_name,
                d.missing,
                d.repos.join(", ")
            )
        })
        .collect();
    Some(Alert {
        severity: AlertSeverity::Info,
        title: if n == 1 {
            "1 agent holds GitHub permissions the installation has not accepted".into()
        } else {
            format!("{n} agents hold GitHub permissions the installation has not accepted")
        },
        detail: format!(
            "{}. An admin gave {} in Talaria, and the tokens of those repos carry the rest without {} until the GitHub App asks for {} and an owner of the account accepts the change on GitHub (Agent policy → GitHub access shows each agent's set beside what the installation holds, and where an owner accepts it).",
            each.join("; "),
            if each.len() == 1 { "it" } else { "them" },
            if each.len() == 1 { "it" } else { "them" },
            if each.len() == 1 { "it" } else { "them" }
        ),
        href: GITHUB_ACCESS_HREF,
    })
}

/// How many agent git tokens Talaria revoked in the last hour without
/// GitHub confirming it (`github.token_revoke_failed` rows, each counting
/// the tokens it names). An installation token lives an hour, so older
/// ones have expired.
async fn unconfirmed_revokes(pg: &sqlx::PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "select coalesce(sum(coalesce((after->>'failed')::bigint, 1)), 0)::bigint from audit_log \
         where action = 'github.token_revoke_failed' and created_at > now() - interval '1 hour'",
    )
    .fetch_one(pg)
    .await
}

/// The alert for tokens revoked without GitHub confirming it: they were
/// dropped from every cache, but GitHub may still honour them until they
/// expire. None when there are none.
pub fn revoke_failed_alert(n: i64) -> Option<Alert> {
    (n > 0).then(|| Alert {
        severity: AlertSeverity::Warning,
        title: if n == 1 {
            "A GitHub token Talaria revoked was not confirmed revoked".into()
        } else {
            format!("{n} GitHub tokens Talaria revoked were not confirmed revoked")
        },
        detail: format!(
            "In the last hour Talaria took back {} from agents whose GitHub access narrowed, and GitHub did not confirm the revoke. Talaria will not hand {} out again, but GitHub may honour {} until {} expire{}, within the hour. The audit trail (github.token_revoke_failed) names the agent.",
            if n == 1 { "a token" } else { "tokens" },
            if n == 1 { "it" } else { "them" },
            if n == 1 { "it" } else { "them" },
            if n == 1 { "it" } else { "they" },
            if n == 1 { "s" } else { "" }
        ),
        href: GIT_HOSTS_HREF,
    })
}

/// One granted repo an agent's git got no token for in the last hour, and
/// the git host's sentence (`git.credential_refused` rows, one per agent,
/// host, repo and sentence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedCredential {
    /// The agent's display name, or its model when the agent is gone.
    pub agent: String,
    /// The git host's name, or its id when the host is gone.
    pub host: String,
    pub repo: String,
    pub message: String,
}

/// The refusals of the last hour, newest first. The door records one per
/// (agent, repo, sentence) per ten minutes, so this is a short list.
async fn refused_credentials(pg: &sqlx::PgPool) -> Result<Vec<RefusedCredential>, sqlx::Error> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "select coalesce(d.display_name, a.actor), \
                coalesce(h.name, a.after->>'host_id', ''), \
                coalesce(a.after->>'repo', ''), coalesce(a.after->>'message', '') \
         from audit_log a \
         left join agent_defs d on d.id::text = a.target_id \
         left join git_hosts h on h.id = a.after->>'host_id' \
         where a.action = 'git.credential_refused' and a.created_at > now() - interval '1 hour' \
         group by 1, 2, 3, 4 order by max(a.created_at) desc limit 20",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(agent, host, repo, message)| RefusedCredential {
            agent,
            host,
            repo,
            message,
        })
        .collect())
}

/// How many refusals the alert names before it counts the rest.
const REFUSALS_NAMED: usize = 5;

/// The alert for granted repos an agent's git got no token for: which agents
/// and repos, and the git host's own words (GitHub refused the mint, the
/// token came back wider than asked, the installation holds none of the
/// agent's set). None when there are none.
pub fn refused_credentials_alert(rows: &[RefusedCredential]) -> Option<Alert> {
    if rows.is_empty() {
        return None;
    }
    let mut agents: Vec<&str> = rows.iter().map(|r| r.agent.as_str()).collect();
    agents.sort_unstable();
    agents.dedup();
    let mut hosts: Vec<&str> = rows.iter().map(|r| r.host.as_str()).collect();
    hosts.sort_unstable();
    hosts.dedup();
    // The host's own name when every refusal is on one host, so a Forgejo
    // host's refusal is never called a GitHub token.
    let token = match hosts.as_slice() {
        [one] if !one.is_empty() => format!("{one} token"),
        _ => "git token".to_string(),
    };
    let mut each: Vec<String> = rows
        .iter()
        .take(REFUSALS_NAMED)
        .map(|r| format!("{} on {} ({}): {}", r.agent, r.repo, r.host, r.message))
        .collect();
    if rows.len() > REFUSALS_NAMED {
        each.push(format!("and {} more", rows.len() - REFUSALS_NAMED));
    }
    Some(Alert {
        severity: AlertSeverity::Warning,
        title: match agents.as_slice() {
            [one] => format!("{one} got no {token} for a granted repo"),
            all => format!("{} agents got no {token} for a granted repo", all.len()),
        },
        detail: format!(
            "In the last hour: {}. The agent's git was told the same, and its push or API call failed. Admin → Git hosts shows each host's connection, and for a GitHub App what each installation holds and where an owner accepts a permission; the audit trail (git.credential_refused) has each one.",
            each.join("; ")
        ),
        href: GIT_HOSTS_HREF,
    })
}

/// Where the unpriced alert sends an admin: the Cost tab's unpriced panel.
pub const UNPRICED_HREF: &str = "/observability/cost#unpriced";

/// The "Unpriced cloud usage" warning for the window's unpriced cloud tokens,
/// or None when there are none — the whole decision, so a test can hold the
/// alert to the ledger number that clears it.
pub fn unpriced_alert(unpriced_cloud_tokens: i64) -> Option<Alert> {
    (unpriced_cloud_tokens > 0).then(|| Alert {
        severity: AlertSeverity::Warning,
        title: "Unpriced cloud usage".into(),
        detail: format!(
            "{} cloud tokens (30d) have no price, so spend is understated. Set a price, mark the model included in a plan, or retry the price lookup.",
            fmt(unpriced_cloud_tokens)
        ),
        href: UNPRICED_HREF,
    })
}

async fn is_admin(pg: &sqlx::PgPool, user_id: &str) -> bool {
    sqlx::query_scalar::<_, String>("select role from users where id::text = $1")
        .bind(user_id)
        .fetch_optional(pg)
        .await
        .ok()
        .flatten()
        .is_some_and(|r| r == "admin")
}

/// Labels of the enabled external servers whose last probe is not `ok` —
/// the same scope the health pass asks (no builtin, app or Workbench row;
/// an http(s) address). A never-probed server is not counted: nothing has
/// said it is broken yet.
async fn mcp_servers_needing_attention(pg: &sqlx::PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "select label from mcp_servers \
         where enabled and not builtin and app_slug is null and name <> 'workbench' \
           and url ~* '^https?://' and last_probe is not null \
           and coalesce(last_probe->>'state', 'ok') <> 'ok' \
         order by label",
    )
    .fetch_all(pg)
    .await
    .unwrap_or_default()
}

async fn probe_mcp() -> bool {
    let url = format!("http://127.0.0.1:{}/mcp", mcp_port());
    let client = reqwest::Client::new();
    match client
        .post(&url)
        .timeout(std::time::Duration::from_millis(2_500))
        .send()
        .await
    {
        Ok(r) => r.status().as_u16() == 401 || r.status().is_success(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_git_hosts::{Level, PermissionSet};

    #[test]
    fn the_unpriced_alert_names_the_tokens_and_links_the_panel_that_fixes_them() {
        assert!(unpriced_alert(0).is_none(), "nothing unpriced, no alert");
        let alert = unpriced_alert(1_234_567).expect("unpriced tokens raise it");
        assert_eq!(alert.severity, AlertSeverity::Warning);
        assert_eq!(alert.title, "Unpriced cloud usage");
        assert_eq!(alert.href, "/observability/cost#unpriced");
        assert!(alert.detail.starts_with(&fmt(1_234_567)));
        // It no longer sends anyone to /models, where no price can be set.
        assert!(!alert.detail.contains("/models"));
    }

    fn member(id: &str, name: &str, renders: bool) -> talaria_agent_defs::DepartmentMember {
        talaria_agent_defs::DepartmentMember {
            id: id.into(),
            slug: id.into(),
            display_name: name.into(),
            renders,
        }
    }

    #[test]
    fn a_shared_department_is_a_warning_that_names_who_runs_and_who_is_left_out() {
        let share = talaria_agent_defs::SharedDepartment {
            department: "research".into(),
            members: vec![member("a", "Ava", true), member("z", "Zed", true)],
        };
        let alert = shared_department_alert(&share);
        assert_eq!(alert.severity, AlertSeverity::Warning);
        assert_eq!(alert.title, "2 agents share the department \"research\"");
        assert_eq!(
            alert.detail,
            "Ava runs in its container (agent-research) and keeps its memory; Zed is left out of the fleet. Retire and delete one of them — Talaria never merges two agents into one. If agent-research still runs Zed (an install upgraded with the pair in place), Roll Ava to put it back in its container."
        );
        assert_eq!(alert.href, "/agents");

        let idle = talaria_agent_defs::SharedDepartment {
            department: "ops".into(),
            members: vec![
                member("a", "Ava", false),
                member("b", "Bo", false),
                member("c", "Cy", false),
            ],
        };
        let alert = shared_department_alert(&idle);
        assert_eq!(alert.title, "3 agents share the department \"ops\"");
        assert!(
            alert.detail.starts_with("None of them is running."),
            "{}",
            alert.detail
        );
    }

    #[test]
    fn mcp_attention_is_one_warning_naming_every_server() {
        assert!(mcp_attention_alert(&[]).is_none());
        let one = mcp_attention_alert(&["DeepWiki".into()]).expect("one server alerts");
        assert_eq!(one.severity, AlertSeverity::Warning);
        assert_eq!(one.title, "1 MCP server needs attention");
        assert_eq!(one.href, "/mcp/external");
        assert!(
            one.detail
                .starts_with("DeepWiki. The last health check could not use it,")
        );
        let two = mcp_attention_alert(&["A".into(), "B".into()]).expect("two servers alert");
        assert_eq!(two.title, "2 MCP servers need attention");
        assert!(two.detail.starts_with(
            "A, B. The last health check could not use them, so agents calling their tools"
        ));
    }

    /// The host row the alert names. Which installations it follows is
    /// `HostHoldings::selected`, so its config is never read here.
    fn github_host() -> talaria_git_hosts::HostRow {
        talaria_git_hosts::HostRow {
            id: "github".into(),
            kind: talaria_git_hosts::HostKind::Github,
            name: "GitHub".into(),
            base_url: "https://github.com".into(),
            api_url: None,
            auth: Default::default(),
            repo_creation_orgs: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn holdings(selected: &[&str], rows: Vec<InstallationRow>) -> HostHoldings {
        HostHoldings {
            host: github_host(),
            selected: selected.iter().map(|s| s.to_string()).collect(),
            rows,
        }
    }

    fn perms(pairs: &[(&str, Level)]) -> PermissionSet {
        PermissionSet::of(pairs)
    }

    /// A good read of installation `id`: the App asks for contents and
    /// metadata, and the installation took both.
    fn checked(id: &str, account: &str, at: i64) -> InstallationRow {
        let all = perms(&[("contents", Level::Write), ("metadata", Level::Read)]);
        InstallationRow {
            host_id: "github".into(),
            installation_id: id.into(),
            account: account.into(),
            app_permissions: Some(all.clone()),
            held: Some(all),
            checked_at_ms: Some(at),
            ..InstallationRow::default()
        }
    }

    const NOW: i64 = 10_000_000_000;

    #[test]
    fn the_github_alerts_read_the_record_and_a_missing_row_is_unknown() {
        // 91 was never read (no row); 92 is read and has nothing waiting.
        let hosts = [holdings(
            &["91", "92"],
            vec![checked("92", "beta", NOW - 60_000)],
        )];
        let alerts = git_host_alerts(&hosts, NOW);
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert_eq!(
            alerts[0].title,
            "GitHub permissions unknown for 1 installation"
        );
        assert!(
            alerts[0]
                .detail
                .starts_with("installation 91 on GitHub: never checked."),
            "{}",
            alerts[0].detail
        );
        assert_eq!(alerts[0].href, "/admin/git");
        // Everything read and accepted: silence is earned, not assumed.
        let quiet = [holdings(&["92"], vec![checked("92", "beta", NOW - 60_000)])];
        assert!(git_host_alerts(&quiet, NOW).is_empty());
        assert!(git_host_alerts(&[], NOW).is_empty());
    }

    /// The App now asks for workflows too, which acme's owners have not
    /// accepted.
    fn asks_for_workflows(mut r: InstallationRow) -> InstallationRow {
        r.app_permissions = Some(perms(&[
            ("contents", Level::Write),
            ("metadata", Level::Read),
            ("workflows", Level::Write),
        ]));
        r
    }

    #[test]
    fn a_change_no_owner_accepted_is_named_with_where_to_accept_it() {
        let mut acme = asks_for_workflows(checked("91", "acme", NOW - 60_000));
        acme.html_url =
            Some("https://github.com/organizations/acme/settings/installations/91".into());
        let alerts = git_host_alerts(&[holdings(&["91"], vec![acme])], NOW);
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert_eq!(
            alerts[0].title,
            "A GitHub App permission change is waiting for org-owner approval on GitHub"
        );
        assert_eq!(
            alerts[0].detail,
            "acme (installation 91 on GitHub) has not accepted workflows:write (an owner accepts it at https://github.com/organizations/acme/settings/installations/91). The App asks for it, and until an owner of the account accepts it on GitHub, Talaria mints tokens without it."
        );
        assert_eq!(alerts[0].href, "/admin/git");
    }

    #[test]
    fn a_failed_or_stale_read_is_unknown_and_its_last_answer_still_counts() {
        let mut failed = asks_for_workflows(checked("91", "acme", NOW - 60_000));
        failed.error = Some("GitHub GET /app/installations/91 → 502: Server Error".into());
        let stale = checked("92", "beta", NOW - STALE_AFTER_MS - 120_000);
        let alerts = git_host_alerts(&[holdings(&["91", "92"], vec![failed, stale])], NOW);
        assert_eq!(alerts.len(), 2, "{alerts:?}");
        // The last good read of 91 still has workflows waiting.
        assert!(
            alerts[0]
                .detail
                .starts_with("acme (installation 91 on GitHub) has not accepted workflows:write."),
            "{}",
            alerts[0].detail
        );
        assert_eq!(
            alerts[1].title,
            "GitHub permissions unknown for 2 installations"
        );
        assert!(
            alerts[1].detail.contains(
                "acme (installation 91 on GitHub): the last check failed (GitHub GET /app/installations/91 → 502: Server Error)"
            ),
            "{}",
            alerts[1].detail
        );
        assert!(
            alerts[1]
                .detail
                .contains("beta (installation 92 on GitHub): not checked for 32 minutes"),
            "{}",
            alerts[1].detail
        );
    }

    #[test]
    fn agents_given_what_the_installation_has_not_accepted_are_named_with_their_repos() {
        use talaria_git_hosts::permissions::Dormant;
        assert!(dormant_permissions_alert(&[]).is_none());
        let wf = perms(&[("workflows", Level::Write)]);
        let dormant = |agent: &str, name: &str, repos: &[&str]| Dormant {
            agent_id: agent.into(),
            agent_name: name.into(),
            host_id: "github".into(),
            host_name: "GitHub".into(),
            missing: wf.clone(),
            repos: repos.iter().map(|r| r.to_string()).collect(),
        };
        let one = dormant_permissions_alert(&[dormant("a1", "Dev", &["acme/site"])]).unwrap();
        assert_eq!(
            one.severity,
            AlertSeverity::Info,
            "informational: nothing waits on it"
        );
        assert_eq!(
            one.title,
            "1 agent holds GitHub permissions the installation has not accepted"
        );
        assert!(
            one.detail
                .starts_with("Dev on GitHub: workflows:write (acme/site)."),
            "{}",
            one.detail
        );
        // About what agents' tokens carry: it opens their GitHub access.
        assert_eq!(one.href, "/admin/github");
        assert!(
            one.detail.contains("Agent policy → GitHub access")
                && !one.detail.contains("Git hosts"),
            "{}",
            one.detail
        );
        let two = dormant_permissions_alert(&[
            dormant("a1", "Dev", &["acme/site"]),
            dormant("a2", "Ana", &["acme/app", "acme/api"]),
        ])
        .unwrap();
        assert_eq!(
            two.title,
            "2 agents hold GitHub permissions the installation has not accepted"
        );
        assert!(
            two.detail
                .contains("Ana on GitHub: workflows:write (acme/app, acme/api)")
        );
    }

    #[test]
    fn a_revoke_github_did_not_confirm_is_said_until_its_token_has_expired() {
        assert!(revoke_failed_alert(0).is_none());
        let one = revoke_failed_alert(1).unwrap();
        assert_eq!(
            one.title,
            "A GitHub token Talaria revoked was not confirmed revoked"
        );
        assert!(one.detail.contains("until it expires"), "{}", one.detail);
        let three = revoke_failed_alert(3).unwrap();
        assert_eq!(
            three.title,
            "3 GitHub tokens Talaria revoked were not confirmed revoked"
        );
        assert!(
            three.detail.contains("until they expire,"),
            "{}",
            three.detail
        );
    }

    #[test]
    fn a_granted_repo_with_no_token_is_named_with_the_hosts_words() {
        assert!(refused_credentials_alert(&[]).is_none());
        let refused = |agent: &str, host: &str, repo: &str, message: &str| RefusedCredential {
            agent: agent.into(),
            host: host.into(),
            repo: repo.into(),
            message: message.into(),
        };
        let one = refused_credentials_alert(&[refused(
            "Ana",
            "GitHub",
            "acme/site",
            "GitHub minted a token for acme/site wider than Talaria asked for (it carries workflows:write, above the read asked for); it was revoked and never used",
        )])
        .unwrap();
        assert_eq!(one.severity, AlertSeverity::Warning);
        assert_eq!(one.title, "Ana got no GitHub token for a granted repo");
        assert!(
            one.detail
                .contains("Ana on acme/site (GitHub): GitHub minted a token for acme/site wider"),
            "{}",
            one.detail
        );
        assert_eq!(one.href, GIT_HOSTS_HREF);
        // A Forgejo host's refusal is called by its host's name, never GitHub.
        let forge = refused_credentials_alert(&[refused(
            "Dev",
            "Forge",
            "acme/app",
            "the git host Forge has no usable login right now",
        )])
        .unwrap();
        assert_eq!(forge.title, "Dev got no Forge token for a granted repo");
        let many: Vec<RefusedCredential> = (0..7)
            .map(|i| {
                refused(
                    if i % 2 == 0 { "Ana" } else { "Dev" },
                    if i < 6 { "GitHub" } else { "Forge" },
                    &format!("acme/r{i}"),
                    "x",
                )
            })
            .collect();
        let two = refused_credentials_alert(&many).unwrap();
        assert_eq!(two.title, "2 agents got no git token for a granted repo");
        assert!(two.detail.contains("and 2 more"), "{}", two.detail);
    }

    #[test]
    fn minutes_matches_js_round() {
        assert_eq!(minutes(0), 0);
        assert_eq!(minutes(89_999), 1); // round(1.4999) = 1
        assert_eq!(minutes(90_000), 2); // round(1.5) = 2
    }
}

/// LIVE: the unpriced alert clears for HISTORY, not only new usage, the
/// moment a price lands — rows booked through the relay's own writer, the
/// real priced view, the real `cost_overview`. `#[ignore]`d; ci.yml's api
/// job runs it with the ignored lib tests in its live-DB steps.
///
///   DATABASE_URL=postgres://… cargo test -p talaria-alerts --lib -- --ignored live
#[cfg(test)]
mod live {
    use super::*;
    use talaria_gateway::usage::{
        AdminPricing, RELAY_SOURCE, StatedFlow, TokenCounts, record_relay_usage,
    };
    use talaria_price_oracle::unpriced::{PriceInput, set_price};

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn the_unpriced_alert_clears_for_history_when_a_price_lands() {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        let pg = sqlx::PgPool::connect(&url).await.expect("connect");
        let ep = format!("alerts-live-{}", now_ms());
        let model = "gpt-4.1-2025-04-14";
        // Rows already in the window, booked before anyone priced the model.
        for prompt in [40_000, 2_000] {
            record_relay_usage(
                &pg,
                RELAY_SOURCE,
                "alerts-live:caller",
                None,
                &StatedFlow::default(),
                &ep,
                "cloud",
                model,
                &TokenCounts {
                    prompt_tokens: prompt,
                    completion_tokens: 1_000,
                    ..TokenCounts::default()
                },
                false,
                None,
            )
            .await
            .expect("the relay's insert");
        }
        let mine = 40_000 + 2_000 + 2 * 1_000;
        let before = cost_overview(&pg)
            .await
            .unwrap()
            .totals
            .unpriced_cloud_tokens;
        // Anything else in this database that is unpriced stays counted; the
        // claim is about these rows only.
        let others = before - mine;
        assert!(others >= 0, "the window counts our {mine} tokens");
        let alert = unpriced_alert(before).expect("our rows raise the alert");
        assert_eq!(alert.href, UNPRICED_HREF);

        set_price(
            &pg,
            &PriceInput {
                endpoint: ep.clone(),
                model: model.into(),
                mode: AdminPricing::Manual,
                price_in_per_mtok: Some(2.0),
                price_out_per_mtok: Some(8.0),
                price_cache_write_per_mtok: None,
                price_cache_read_per_mtok: None,
                note: None,
            },
            "alerts-live",
        )
        .await
        .unwrap();
        let after = cost_overview(&pg)
            .await
            .unwrap()
            .totals
            .unpriced_cloud_tokens;
        assert_eq!(after, others, "every past row of the model is priced now");
        assert_eq!(
            unpriced_alert(after).is_some(),
            others > 0,
            "the alert is gone unless something else is still unpriced"
        );
        for sql in [
            "delete from usage_events where endpoint = $1",
            "delete from model_price_overrides where endpoint = $1",
        ] {
            sqlx::query(sql).bind(&ep).execute(&pg).await.unwrap();
        }
    }

    /// The refused-credential alert's read against the real trail: a
    /// `git.credential_refused` row written the way the door writes it is
    /// named with the agent's and the host's names, a gone agent by its
    /// model and a gone host by its id, and a row older than an hour is not.
    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn a_refused_git_credential_is_read_with_the_agents_and_hosts_names() {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        let pg = sqlx::PgPool::connect(&url).await.expect("connect");
        let t = format!("alerts-refused-{}", now_ms());
        let agent: String = sqlx::query_scalar(
            "insert into agent_defs (slug, department, model, display_name, developer) \
             values ($1, $1, $1, $2, true) returning id::text",
        )
        .bind(&t)
        .bind(format!("Refused {t}"))
        .fetch_one(&pg)
        .await
        .expect("the agent");
        sqlx::query(
            "insert into git_hosts (id, kind, name, base_url, auth) \
             values ($1, 'github', $2, $3, '{}'::jsonb)",
        )
        .bind(&t)
        .bind(format!("Host {t}"))
        .bind(format!("https://{t}.example.invalid"))
        .execute(&pg)
        .await
        .expect("the host");
        let refused = |actor: &str, target: &str, host_id: &str, repo: &str, age: &str| {
            let (pg, actor, target, host_id, repo, age) = (
                pg.clone(),
                actor.to_string(),
                target.to_string(),
                host_id.to_string(),
                repo.to_string(),
                age.to_string(),
            );
            async move {
                // The after-data the door's `refused_after` writes.
                sqlx::query(
                    "insert into audit_log (actor, action, target_type, target_id, target_label, after, created_at) \
                     values ($1, 'git.credential_refused', 'agent', $2, $3, \
                             jsonb_build_object('source', 'git_host', 'host', 'github.com', \
                               'host_id', $4::text, 'repo', $5::text, 'kind', 'refused', \
                               'message', $6::text, \
                               'asked', '{\"contents\":\"write\",\"metadata\":\"read\"}'::jsonb, \
                               'shortfall', '{}'::jsonb), \
                             now() - $7::interval)",
                )
                .bind(&actor)
                .bind(&target)
                .bind(format!("{host_id}:{repo}"))
                .bind(&host_id)
                .bind(&repo)
                .bind(format!("GitHub refused a token for {repo}"))
                .bind(age)
                .execute(&pg)
                .await
                .expect("the row");
            }
        };
        let gone_agent = "00000000-0000-4000-8000-00000000a1e7";
        refused(&t, &agent, &t, "acme/one", "1 minute").await;
        refused(&t, &agent, &format!("{t}-gone"), "acme/two", "2 minutes").await;
        refused(
            &format!("{t}-model"),
            gone_agent,
            &t,
            "acme/three",
            "3 minutes",
        )
        .await;
        refused(&t, &agent, &t, "acme/old", "2 hours").await;

        let rows = refused_credentials(&pg).await.expect("the read");
        let mine: Vec<&RefusedCredential> = rows
            .iter()
            .filter(|r| r.repo.starts_with("acme/") && r.message.starts_with("GitHub refused"))
            .filter(|r| r.agent.contains(&t) || r.host.contains(&t))
            .collect();
        let named = |repo: &str| mine.iter().find(|r| r.repo == repo).copied();
        let one = named("acme/one").expect("the newest row");
        assert_eq!(one.agent, format!("Refused {t}"));
        assert_eq!(one.host, format!("Host {t}"));
        assert_eq!(one.message, "GitHub refused a token for acme/one");
        assert_eq!(
            named("acme/two").map(|r| r.host.clone()),
            Some(format!("{t}-gone")),
            "a gone host by its id"
        );
        assert_eq!(
            named("acme/three").map(|r| r.agent.clone()),
            Some(format!("{t}-model")),
            "a gone agent by its model"
        );
        assert!(named("acme/old").is_none(), "older than an hour: {mine:?}");
        assert_eq!(mine[0].repo, "acme/one", "newest first: {mine:?}");
        let alert = refused_credentials_alert(&rows).expect("the alert");
        assert_eq!(alert.href, GIT_HOSTS_HREF);

        sqlx::query("delete from audit_log where actor in ($1, $2)")
            .bind(&t)
            .bind(format!("{t}-model"))
            .execute(&pg)
            .await
            .unwrap();
        sqlx::query("delete from agent_defs where id = $1::uuid")
            .bind(&agent)
            .execute(&pg)
            .await
            .unwrap();
        sqlx::query("delete from git_hosts where id = $1")
            .bind(&t)
            .execute(&pg)
            .await
            .unwrap();
    }
}
