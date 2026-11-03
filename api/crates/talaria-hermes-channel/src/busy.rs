// WHETHER AN AGENT IS IN USE — the rollout restarts an agent, so it passes
// over one that is working or about to, and comes back to it later (and, if
// it is busy all window, leaves it for a later window rather than hold the
// rollout open).
//
// Asked of the agent itself first, in the fields Hermes serves — read from
// the source of both v0.22.4 and v0.22.6 (gateway/platforms/api_server.py
// `_handle_health_detailed`, gateway/readiness.py, cron/jobs.py), and held to
// by every new image's canary (probe.rs), so a Hermes that moves them is held
// for an admin rather than read as idle:
//
//   /health/detailed  `api_server.active_runs` — live: every
//                     /v1/chat/completions and /v1/runs turn in flight;
//                     `readiness.checks.background_queues.active_api_runs`
//                     and `.active_delegations` — live; `active_agents` and
//                     `gateway_busy` — the gateway's last snapshot (a
//                     messaging-platform turn is written into it when it
//                     starts): read as busy when set, never as idle.
//   /api/jobs         a job whose `fire_claim.at` was refreshed in the last
//                     few minutes is RUNNING — Hermes heartbeats the claim
//                     every 60 s for as long as the run lives, a long tool
//                     call included, and calls a claim older than 300 s dead
//                     (`FIRE_CLAIM_TTL_SECONDS`); v0.22.6's `live_execution`
//                     says the same. A job due in the next few minutes is
//                     about to run — unless it runs every few minutes: Hermes
//                     runs a fire it missed while the agent restarted late,
//                     within its grace window (at least 120 s), so a
//                     short-interval job never pins an agent busy.
//
// An answer Talaria cannot read — none, a non-2xx, a shape it does not know —
// is UNKNOWN, and an agent whose work is unknown is not restarted now: never
// read as idle. Then the database, so every api instance answers the same: a
// reply being written (inside its writer's lease), a channel reply being
// written, a live work session, a started workbench job, a message in one of
// its conversations in the last few minutes, or a model call completed in
// the last few minutes (`usage_events`).

use sqlx::PgPool;

use crate::ops::ChannelAgent;

/// A conversation with a message this recent is a person mid-conversation.
pub const RECENT_CHAT_MINUTES: i32 = 10;

/// A model call finished this recently is the agent working. (A call still in
/// flight is a run Hermes reports in `/health/detailed`. The gateway key's
/// `last_used_at` is NOT a signal: Hermes lists its models through it at
/// every boot, so every freshly started agent would read as busy — measured
/// live, and it held a rollback back from an agent on a bad image.)
pub const RECENT_CALL_MINUTES: i32 = 5;

/// A cron run's claim refreshed this recently is a run in progress: Hermes'
/// own liveness (300 s), plus a minute for the heartbeat in flight.
pub const CLAIM_LIVE_MS: i64 = 6 * 60 * 1000;

/// A scheduled job due within this long is about to run: a roll (drain,
/// recreate, boot) takes about two minutes.
pub const DUE_SOON_MS: i64 = 3 * 60 * 1000;

/// A job that runs at least this often is never "about to run": it would pin
/// the agent busy forever, and a fire missed during the roll runs late.
pub const FREQUENT_MS: i64 = 10 * 60 * 1000;

/// What the database says, in order of how busy it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DbSignals {
    pub streaming: bool,
    pub channel: bool,
    pub work: bool,
    pub workbench: bool,
    pub chat: bool,
    pub call: bool,
}

/// The database's signals for one agent.
pub async fn db_signals(pg: &PgPool, agent: &ChannelAgent) -> Result<DbSignals, sqlx::Error> {
    let row: (bool, bool, bool, bool, bool, bool) = sqlx::query_as(
        "select \
           exists (select 1 from messages m join conversations c on c.id = m.conversation_id \
                   where c.agent_model = $1 and m.status = 'streaming' \
                     and m.streamed_at > now() - interval '450 seconds'), \
           exists (select 1 from channel_messages cm \
                   where cm.author_type = 'agent' and cm.author = $1 and cm.status = 'streaming' \
                     and cm.created_at > now() - interval '15 minutes'), \
           exists (select 1 from runs r where r.kind = 'work-session' \
                   and r.state in ('queued', 'running', 'awaiting') \
                   and r.input->>'agentModel' = $1), \
           exists (select 1 from workbench_jobs w where w.agent_model = $1 \
                   and w.status = 'started'), \
           exists (select 1 from messages m join conversations c on c.id = m.conversation_id \
                   where c.agent_model = $1 \
                     and m.created_at > now() - make_interval(mins => $2)), \
           exists (select 1 from usage_events u where u.agent_model = $1 \
                   and u.created_at > now() - make_interval(mins => $3))",
    )
    .bind(&agent.model)
    .bind(RECENT_CHAT_MINUTES)
    .bind(RECENT_CALL_MINUTES)
    .fetch_one(pg)
    .await?;
    Ok(DbSignals {
        streaming: row.0,
        channel: row.1,
        work: row.2,
        workbench: row.3,
        chat: row.4,
        call: row.5,
    })
}

/// The sentence for what the database found.
pub fn why_busy(s: DbSignals) -> Option<String> {
    if s.streaming {
        Some("it is writing a reply".into())
    } else if s.channel {
        Some("it is writing a channel reply".into())
    } else if s.work {
        Some("it has a work session in progress".into())
    } else if s.workbench {
        Some("it has a workbench job in progress".into())
    } else if s.chat {
        Some(format!(
            "a conversation with it had a message in the last {RECENT_CHAT_MINUTES} minutes"
        ))
    } else if s.call {
        Some(format!(
            "it finished a model call in the last {RECENT_CALL_MINUTES} minutes"
        ))
    } else {
        None
    }
}

/// What Hermes' `/health/detailed` says of the work in hand: `Ok(None)`
/// idle, `Ok(Some)` busy and why, `Err` a shape Talaria cannot read.
pub fn hermes_busy(body: &serde_json::Value) -> Result<Option<String>, String> {
    let runs = body
        .pointer("/api_server/active_runs")
        .and_then(|v| v.as_i64())
        .ok_or("it has no api_server.active_runs, the count of turns in flight")?;
    let queues = body
        .pointer("/readiness/checks/background_queues")
        .filter(|v| v.is_object())
        .ok_or("it has no readiness.checks.background_queues")?;
    let count = |v: Option<&serde_json::Value>| v.and_then(|v| v.as_i64()).unwrap_or(0);
    let queued = count(queues.get("active_api_runs"));
    let delegated = count(queues.get("active_delegations"));
    let agents = count(body.get("active_agents"));
    let gateway = body.get("gateway_busy").and_then(|v| v.as_bool()) == Some(true);
    let plural = |n: i64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    Ok(if runs > 0 || queued > 0 {
        Some(format!(
            "Hermes reports {} in flight",
            plural(runs.max(queued), "turn", "turns")
        ))
    } else if delegated > 0 {
        Some(format!(
            "Hermes reports {} running",
            plural(delegated, "delegated task", "delegated tasks")
        ))
    } else if agents > 0 || gateway {
        Some("Hermes reports its gateway at work".to_string())
    } else {
        None
    })
}

/// A time as Hermes writes one (RFC 3339, with or without an offset; none is
/// UTC), in epoch ms.
pub fn parse_when(s: &str) -> Option<i64> {
    let s = s.trim();
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis())
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|n| n.and_utc().timestamp_millis())
        })
}

/// Why the agent's schedule makes it busy, from Hermes' `/api/jobs` body: a
/// job running now, or one about to. `Err`: a shape Talaria cannot read.
pub fn cron_busy(body: &serde_json::Value, now_ms: i64) -> Result<Option<String>, String> {
    let jobs = body
        .get("jobs")
        .and_then(|v| v.as_array())
        .ok_or("it did not answer {\"jobs\": [...]}")?;
    let when = |j: &serde_json::Value, ptr: &str| {
        j.pointer(ptr).and_then(|v| v.as_str()).and_then(parse_when)
    };
    for job in jobs {
        if job.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            continue;
        }
        let name = job
            .get("name")
            .and_then(|v| v.as_str())
            .or_else(|| job.get("id").and_then(|v| v.as_str()))
            .unwrap_or("a job")
            .chars()
            .take(80)
            .collect::<String>();
        let claimed = when(job, "/fire_claim/at")
            .is_some_and(|at| at > now_ms - CLAIM_LIVE_MS && at < now_ms + 60_000);
        let live = job.get("live_execution").is_some_and(|v| v.is_object());
        if claimed || live {
            return Ok(Some(format!("its scheduled job \"{name}\" is running")));
        }
        if let Some(at) = when(job, "/next_run_at")
            && at >= now_ms - 60_000
            && at <= now_ms + DUE_SOON_MS
            && !when(job, "/last_run_at").is_some_and(|last| at - last < FREQUENT_MS)
        {
            return Ok(Some(format!(
                "its scheduled job \"{name}\" is due in {} min",
                ((at - now_ms).max(0) + 59_999) / 60_000
            )));
        }
    }
    Ok(None)
}

/// Not restarted now: what it is doing cannot be read.
fn unknown(why: impl std::fmt::Display) -> Option<String> {
    Some(format!(
        "whether it is working could not be read ({why}), so it is not restarted now"
    ))
}

/// Why the agent is in use, or None. An answer that cannot be read is a
/// reason too (`unknown`), never idle.
pub async fn busy(pg: &PgPool, agent: &ChannelAgent) -> Result<Option<String>, sqlx::Error> {
    let Some(entry) = talaria_fleet_layout::read_manifest()
        .await
        .into_iter()
        .find(|e| e.model == agent.model)
    else {
        return Ok(unknown("it is not in the fleet manifest"));
    };
    let base = entry.url.trim_end_matches('/').to_string();
    let key = entry.key.as_deref();
    match crate::probe::get_json(&format!("{base}/health/detailed"), key).await {
        Ok((200, body)) => match hermes_busy(&body) {
            Ok(Some(why)) => return Ok(Some(why)),
            Ok(None) => {}
            Err(e) => return Ok(unknown(format!("/health/detailed: {e}"))),
        },
        Ok((status, _)) => return Ok(unknown(format!("/health/detailed answered {status}"))),
        Err(e) => return Ok(unknown(format!("/health/detailed: {e}"))),
    }
    match crate::probe::get_json(&format!("{base}/api/jobs?include_disabled=true"), key).await {
        Ok((200, body)) => match cron_busy(&body, talaria_agent_auth::now_ms()) {
            Ok(Some(why)) => return Ok(Some(why)),
            Ok(None) => {}
            Err(e) => return Ok(unknown(format!("/api/jobs: {e}"))),
        },
        // Hermes' answer when its cron module is not there: no jobs to run.
        Ok((501, _)) => {}
        Ok((status, _)) => return Ok(unknown(format!("/api/jobs answered {status}"))),
        Err(e) => return Ok(unknown(format!("/api/jobs: {e}"))),
    }
    Ok(why_busy(db_signals(pg, agent).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured live from the worktree stack's agent (2026-11-03): its idle
    /// `/health/detailed` on v0.22.6 and on v0.22.4, and its `/api/jobs`
    /// a minute into a no-agent job that sleeps — v0.22.6 says
    /// `live_execution`, v0.22.4 only the heartbeated `fire_claim`. While
    /// those jobs ran, `/health/detailed` read idle in every field.
    const HEALTH_V0226: &str =
        include_str!("../tests/fixtures/hermes-v0.22.6-health-detailed.json");
    const HEALTH_V0224: &str =
        include_str!("../tests/fixtures/hermes-v0.22.4-health-detailed.json");
    const JOBS_RUNNING: &str = include_str!("../tests/fixtures/hermes-v0.22.6-jobs-running.json");
    const JOBS_RUNNING_V0224: &str =
        include_str!("../tests/fixtures/hermes-v0.22.4-jobs-running.json");

    fn json(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn the_busiest_reason_is_the_one_said() {
        assert_eq!(why_busy(DbSignals::default()), None);
        let all = DbSignals {
            streaming: true,
            channel: true,
            work: true,
            workbench: true,
            chat: true,
            call: true,
        };
        assert_eq!(why_busy(all).as_deref(), Some("it is writing a reply"));
        let wb = DbSignals {
            workbench: true,
            ..Default::default()
        };
        assert!(why_busy(wb).unwrap().contains("workbench job"));
        let call = DbSignals {
            call: true,
            ..Default::default()
        };
        assert!(why_busy(call).unwrap().contains("finished a model call"));
    }

    #[test]
    fn both_images_health_reads_idle_and_a_turn_in_flight_reads_busy() {
        for body in [HEALTH_V0226, HEALTH_V0224] {
            let mut v = json(body);
            assert_eq!(hermes_busy(&v), Ok(None), "idle as captured");
            v["api_server"]["active_runs"] = serde_json::json!(1);
            assert_eq!(
                hermes_busy(&v).unwrap().as_deref(),
                Some("Hermes reports 1 turn in flight")
            );
            let mut v = json(body);
            v["readiness"]["checks"]["background_queues"]["active_delegations"] =
                serde_json::json!(2);
            assert!(
                hermes_busy(&v)
                    .unwrap()
                    .unwrap()
                    .contains("2 delegated tasks")
            );
        }
    }

    #[test]
    fn a_health_shape_talaria_cannot_read_is_unknown_never_idle() {
        // The shape round two read (`readiness.active_api_runs`), which no
        // Hermes serves, and an answer with nothing in it.
        for body in [
            serde_json::json!({ "gateway_busy": false, "active_agents": 0, "readiness": { "active_api_runs": 0 } }),
            serde_json::json!({ "status": "ok" }),
        ] {
            assert!(hermes_busy(&body).is_err(), "{body}");
        }
    }

    #[test]
    fn a_cron_mid_run_is_busy_from_its_claim_and_a_stale_claim_is_not() {
        let body = json(JOBS_RUNNING);
        let at = body["jobs"][0]["fire_claim"]["at"].as_str().unwrap();
        let claimed = parse_when(at).unwrap();
        // Mid-run: the claim was refreshed a minute ago, and Hermes already
        // moved the job's next run to tomorrow.
        assert!(
            cron_busy(&body, claimed + 60_000)
                .unwrap()
                .unwrap()
                .contains("is running")
        );
        // Ten minutes without a heartbeat: the run died, the job is not busy.
        let mut stale = body.clone();
        stale["jobs"][0]["live_execution"] = serde_json::Value::Null;
        assert_eq!(cron_busy(&stale, claimed + 10 * 60_000), Ok(None));
        // v0.22.4 has no `live_execution`: its claim alone says so.
        let old = json(JOBS_RUNNING_V0224);
        assert!(old["jobs"][0].get("live_execution").is_none());
        let at = parse_when(old["jobs"][0]["fire_claim"]["at"].as_str().unwrap()).unwrap();
        assert!(
            cron_busy(&old, at + 30_000)
                .unwrap()
                .unwrap()
                .contains("\"short-sleeper\" is running")
        );
        assert_eq!(cron_busy(&old, at + 10 * 60_000), Ok(None));
    }

    #[test]
    fn a_job_due_soon_is_busy_unless_it_runs_every_few_minutes() {
        let now = parse_when("2026-11-03T10:00:00Z").unwrap();
        assert_eq!(parse_when("2026-11-03T10:00:00+00:00"), Some(now));
        assert_eq!(parse_when("2026-11-03T03:00:00-07:00"), Some(now));
        assert_eq!(parse_when("2026-11-03T10:00:00.123456"), Some(now + 123));
        let jobs = |next: &str, last: Option<&str>| {
            serde_json::json!({ "jobs": [{ "id": "j1", "name": "nightly", "enabled": true,
                "next_run_at": next, "last_run_at": last, "fire_claim": null }] })
        };
        assert_eq!(
            cron_busy(&jobs("2026-11-03T11:00:00+00:00", None), now),
            Ok(None)
        );
        assert_eq!(
            cron_busy(
                &jobs(
                    "2026-11-03T10:02:30+00:00",
                    Some("2026-11-02T10:02:30+00:00")
                ),
                now
            )
            .unwrap()
            .as_deref(),
            Some("its scheduled job \"nightly\" is due in 3 min")
        );
        // Every five minutes: always due soon, so never a reason to wait.
        assert_eq!(
            cron_busy(
                &jobs(
                    "2026-11-03T10:02:30+00:00",
                    Some("2026-11-03T09:57:30+00:00")
                ),
                now
            ),
            Ok(None)
        );
        let mut paused = jobs("2026-11-03T10:01:00+00:00", None);
        paused["jobs"][0]["enabled"] = serde_json::json!(false);
        assert_eq!(
            cron_busy(&paused, now),
            Ok(None),
            "a paused job runs nothing"
        );
        assert!(cron_busy(&serde_json::json!({ "error": "x" }), now).is_err());
    }
}
