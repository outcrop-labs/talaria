// Derived system alerts — computed from live state on every read, no tables.
// Sources: managed-container reality (docker), gateway reachability, the token
// ledger's blind spots (unpriced cloud usage, estimated counts), the background
// scheduler (failing jobs, and whether it is running at all), the notification
// outbox (depth, age, breaker, what has been lost), and stuck tickets on boards
// the requesting user can access.
//
// THE RULE THIS FILE IS FOR: anything whose failure mode is SILENCE has to be
// asked here, because nothing else asks it. A dead container is obvious the
// moment you use the product; a paused mail queue, a job that stopped running
// and a scheduler that never started are all indistinguishable from a quiet
// week. Every one of those has a live number somewhere in the process — the
// work is bringing it to a person, not computing it.
//
// Port of ui/src/server/alerts.ts. The scheduler and outbox rows describe the
// ANSWERING process's own memory, as in TS — on a multi-instance deployment
// that is whichever instance answered, and under TS/Rust coexistence the two
// genuinely differ (that is the documented divergence, not a bug).

use std::collections::HashMap;

use serde::Serialize;

use crate::boards::board_visibility_sql;
use crate::brain_health::fleet_brain_health;
use crate::fleet::list_agents;
use crate::fleet_docker::container_status;
use crate::fleet_preflight::last_fleet_preflight;
use crate::gateway::budget::group;
use crate::gateway::usage::cost_overview;
use crate::mcp_service::mcp_port;
use crate::notify::notification_mail_stats;
use crate::retrieval::backfill::rag_health;
use crate::retrieval::embed;
use crate::retrieval::migrate::retrieval_upgrade_status;
use crate::retrieval::qdrant;
use crate::scheduler::{scheduler_status, unhealthy_jobs, HealthSeverity};
use crate::state::AppState;

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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// Polled by /alerts (60s) and Home (30s): a short cache keeps repeat loads
// instant, and the probes below run in PARALLEL — serially they added up to
// seconds (docker exec + four network probes).
static ALERTS_CACHE: std::sync::LazyLock<
    tokio::sync::Mutex<HashMap<String, (i64, Vec<Alert>)>>,
> = std::sync::LazyLock::new(|| tokio::sync::Mutex::new(HashMap::new()));
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

/// `n.toLocaleString('en-US')` via the crate's shared grouping helper.
fn fmt(n: i64) -> String {
    group(n)
}

/// `Math.max(1, Math.round(ms / 60_000))` — the alert sentences' minute
/// count. A hair under 90s still reads as "2" (Math.round), never 0.
fn minutes(ms: i64) -> i64 {
    ((ms as f64) / 60_000.0).round() as i64
}

async fn compute_alerts_fresh(state: &AppState, user_id: &str) -> Vec<Alert> {
    let pg = &state.pg;
    let mut alerts: Vec<Alert> = Vec::new();

    // ── Fleet: every enabled managed agent should have a running container ─────
    let managed: Vec<(String, String, String)> = sqlx::query_as(
        "select slug, department, display_name from agent_defs \
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
    let (states, manifest_count, preflight, mcp_up, rag, upgrade, brains, cost, stuck) = tokio::join!(
        states_fut,
        manifest_count_fut,
        preflight_fut,
        mcp_up_fut,
        rag_fut,
        upgrade_fut,
        brains_fut,
        cost_fut,
        stuck_fut
    );
    let stuck = stuck.unwrap_or_default();
    let cost = cost.ok();

    if !managed.is_empty()
        && let Some(states) = states.as_ref()
    {
        for (_, department, display_name) in &managed {
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
                Some(st) if st.status.to_lowercase().contains("unhealthy") => alerts.push(
                    Alert {
                        severity: AlertSeverity::Warning,
                        title: format!("{display_name} is unhealthy"),
                        detail: st.status.clone(),
                        href: "/agents",
                    },
                ),
                _ => {}
            }
        }
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
        let (was, it) = if is_one { ("is", "it") } else { ("are", "them") };
        let model = u.embed.as_ref().map(|e| e.model_id.clone()).unwrap_or_default();
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
                        if one { "tier/fallback is" } else { "tiers/fallbacks are" }
                    ),
                    detail: degraded
                        .iter()
                        .map(|t| {
                            format!(
                                "{}{} → {}/{} ({})",
                                t.kind,
                                t.name.as_deref().map(|n| format!(" \"{n}\"")).unwrap_or_default(),
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
            title: format!("Background job \"{}\" is not running cleanly", job.name.as_str()),
            detail: format!("{} Reported by the instance that served this request.", job.detail),
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
    } else if unarmed == jobs.len()
        && std::env::var("TALARIA_SCHEDULER").as_deref() == Ok("rust")
    {
        // Not an emergency and not a bug: the schedule was handed to the Rust
        // api (TALARIA_SCHEDULER=rust), which registers and arms its own
        // table. This process arming too would double-fire per-instance jobs,
        // so standing down is the healthy state.
        alerts.push(Alert {
            severity: AlertSeverity::Info,
            title: "Background jobs run on the Rust api".into(),
            detail: "TALARIA_SCHEDULER=rust, so this process arms nothing by design. The job table lives in the Rust api; its boot log says what it armed, and the same env value is what told this scheduler to stand down.".into(),
            href: "/observability",
        });
    } else if unarmed == jobs.len() {
        // Registered but never armed. Expected on a developer's machine, so it
        // is only an emergency where a build is actually serving people.
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
                    " Either TALARIA_SCHEDULER=off, or server-entry.js could not find the scheduler handle at boot. The startup log says which."
                } else {
                    " Normal under `vite dev`, which does not run server-entry.js on purpose."
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
            (mail.dropped > 0).then(|| format!("{} refused at the door because the queue was full", fmt(mail.dropped as i64))),
            (mail.abandoned > 0).then(|| format!("{} given up on after repeated send failures", fmt(mail.abandoned as i64))),
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
        if cost.totals.unpriced_cloud_tokens > 0 {
            alerts.push(Alert {
                severity: AlertSeverity::Warning,
                title: "Unpriced cloud usage".into(),
                detail: format!(
                    "{} cloud tokens (30d) have no rate, so spend is understated. Set prices on /models.",
                    fmt(cost.totals.unpriced_cloud_tokens)
                ),
                href: "/models",
            });
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
                href: &leak(format!("/boards/{board_id}/{id}")),
            });
        } else {
            alerts.push(Alert {
                severity: AlertSeverity::Info,
                title: format!("Blocked {days}d: {title}"),
                detail: format!("on {board}: blocked for {days} days"),
                href: &leak(format!("/boards/{board_id}/{id}")),
            });
        }
    }

    // TS's Array#sort with a severity rank — stable, severity-first.
    alerts.sort_by_key(|a| match a.severity {
        AlertSeverity::Critical => 0,
        AlertSeverity::Warning => 1,
        AlertSeverity::Info => 2,
    });
    alerts
}

/// Stuck-ticket hrefs are per-row paths, not literals. `sort_by_key` borrows
/// the vec while comparing, so each Alert keeps its own leaked &'static str —
/// at most twenty per read, freed never, exactly like TS hands out fresh
/// strings nobody frees.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The fleet MCP probe: POST like a tool client, 2.5s budget. Up means 401
/// (auth required — the service is ALIVE) or any 2xx. Everything else,
/// including a timeout, reads as down.
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

    #[test]
    fn minutes_matches_js_round() {
        assert_eq!(minutes(0), 0);
        assert_eq!(minutes(89_999), 1); // round(1.4999) = 1
        assert_eq!(minutes(90_000), 2); // round(1.5) = 2
    }
}
