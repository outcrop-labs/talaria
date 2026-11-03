// WHETHER AN AGENT IS IN USE — the rollout restarts an agent, so it passes
// over one that is working or about to, and comes back to it later (and, if
// it is busy all window, leaves it for a later window rather than hold the
// rollout open).
//
// Asked of the agent itself first: Hermes' `GET /health/detailed` (bearer)
// says whether its gateway is busy and how many agent runs are active —
// a turn in flight, a long tool call, a cron run, a delegated child — and its
// `/api/jobs` says which scheduled jobs run now or soon. Then of the
// database, so every api instance answers the same: a reply being written
// (inside its writer's lease), a channel reply being written, a live work
// session, a started workbench job, a message in one of its conversations in
// the last few minutes, or a model call completed in the last few minutes
// (`usage_events`). An agent that will not answer its own questions is judged
// by the database alone.

use std::time::Duration;

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

/// A scheduled job due within this long is about to run: rolling the agent
/// now would restart it under the job.
pub const CRON_LOOKAHEAD_MS: i64 = 15 * 60 * 1000;

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

/// What Hermes' `/health/detailed` says of the work in hand, or None when
/// it does not say (an older or newer Hermes whose fields differ, or one
/// that will not answer): then the database decides alone.
pub fn hermes_busy(body: &serde_json::Value) -> Option<String> {
    let active = body
        .get("active_agents")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let runs = body
        .pointer("/readiness/active_api_runs")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let busy = body.get("gateway_busy").and_then(|v| v.as_bool()) == Some(true);
    let n = active.max(runs);
    (busy || n > 0).then(|| {
        format!(
            "Hermes reports work in progress ({n} active agent run{}{})",
            if n == 1 { "" } else { "s" },
            if busy { ", gateway busy" } else { "" }
        )
    })
}

/// A cron job's next run, in epoch ms, from Hermes' spelling (RFC 3339, with
/// or without an offset; no offset is UTC).
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

/// Why the agent's schedule makes it busy: a job running now, or one due
/// within [`CRON_LOOKAHEAD_MS`].
pub fn cron_busy(jobs: &[talaria_agent_crons::CronJob], now_ms: i64) -> Option<String> {
    for job in jobs.iter().filter(|j| j.enabled) {
        if job.state == "running" {
            return Some(format!("its scheduled job \"{}\" is running", job.name));
        }
        if let Some(at) = job.next_run_at.as_deref().and_then(parse_when)
            && at >= now_ms - 60_000
            && at <= now_ms + CRON_LOOKAHEAD_MS
        {
            return Some(format!(
                "its scheduled job \"{}\" is due in {} min",
                job.name,
                ((at - now_ms).max(0) + 59_999) / 60_000
            ));
        }
    }
    None
}

/// Ask the agent: its detailed health, over its manifest entry.
async fn ask_hermes(agent: &ChannelAgent) -> Option<String> {
    let entry = talaria_fleet_layout::read_manifest()
        .await
        .into_iter()
        .find(|e| e.model == agent.model)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let mut req = client.get(format!(
        "{}/health/detailed",
        entry.url.trim_end_matches('/')
    ));
    if let Some(key) = entry.key.as_deref() {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: serde_json::Value = resp.json().await.ok()?;
    hermes_busy(&body)
}

/// Why the agent is in use, or None.
pub async fn busy(pg: &PgPool, agent: &ChannelAgent) -> Result<Option<String>, sqlx::Error> {
    if let Some(why) = ask_hermes(agent).await {
        return Ok(Some(why));
    }
    if let Ok(jobs) = talaria_agent_crons::list_cron_jobs(pg, &agent.id).await
        && let Some(why) = cron_busy(&jobs, talaria_agent_auth::now_ms())
    {
        return Ok(Some(why));
    }
    Ok(why_busy(db_signals(pg, agent).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(name: &str, state: &str, next: Option<&str>) -> talaria_agent_crons::CronJob {
        talaria_agent_crons::CronJob {
            id: "abc123".into(),
            name: name.into(),
            prompt: String::new(),
            schedule: "every 60m".into(),
            enabled: true,
            state: state.into(),
            next_run_at: next.map(str::to_string),
            last_run_at: None,
            last_status: None,
            last_error: None,
        }
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
    fn hermes_says_when_it_is_working_and_silence_is_not_idle() {
        // v0.22.6's /health/detailed shape, trimmed.
        let idle = serde_json::json!({ "gateway_busy": false, "active_agents": 0,
            "readiness": { "active_api_runs": 0 } });
        assert_eq!(hermes_busy(&idle), None);
        let working = serde_json::json!({ "gateway_busy": true, "active_agents": 1 });
        assert!(
            hermes_busy(&working)
                .unwrap()
                .contains("1 active agent run")
        );
        let api = serde_json::json!({ "readiness": { "active_api_runs": 2 } });
        assert!(hermes_busy(&api).unwrap().contains("2 active agent runs"));
        assert_eq!(hermes_busy(&serde_json::json!({ "status": "ok" })), None);
    }

    #[test]
    fn a_job_running_or_due_soon_makes_the_agent_busy() {
        let now = parse_when("2026-11-03T10:00:00Z").unwrap();
        assert_eq!(parse_when("2026-11-03T10:00:00+00:00"), Some(now));
        assert_eq!(parse_when("2026-11-03T03:00:00-07:00"), Some(now));
        assert_eq!(parse_when("2026-11-03T10:00:00.123456"), Some(now + 123));
        assert_eq!(
            cron_busy(
                &[job("nightly", "scheduled", Some("2026-11-03T11:00:00Z"))],
                now
            ),
            None
        );
        let due = cron_busy(
            &[job(
                "nightly",
                "scheduled",
                Some("2026-11-03T10:09:30+00:00"),
            )],
            now,
        );
        assert_eq!(
            due.as_deref(),
            Some("its scheduled job \"nightly\" is due in 10 min")
        );
        assert!(
            cron_busy(&[job("sweep", "running", None)], now)
                .unwrap()
                .contains("is running")
        );
        let mut paused = job("off", "scheduled", Some("2026-11-03T10:01:00Z"));
        paused.enabled = false;
        assert_eq!(cron_busy(&[paused], now), None, "a paused job runs nothing");
    }
}
