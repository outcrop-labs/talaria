// The live work status of a ticket's session — what the board and the ticket
// show for "is the agent on it, doing what, for how long, at what cost".
//
// Three reads, all server-owned, so the answer is the same after a refresh,
// in a second tab or on another device:
//
//   - the run row: which turn, and whether the agent has TAKEN that turn
//     (`runs.agent_run_id` is written the moment Hermes names its run, so a
//     null one on a sent turn is a turn still waiting for the agent);
//   - the work pulse (`talaria-work-pulse`): the step running now, since
//     when, what the last step printed, what failed — read off the watch
//     tail every frame already lands on;
//   - the ledger: the session's tokens and cost, priced by the same view
//     the usage pages use. A session whose agent's calls did not land on the
//     ticket says so and why, rather than reading as free.

use std::collections::HashMap;

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_state::AppState;

/// One live session to report on.
pub struct Live {
    pub run_id: String,
    pub task_id: String,
    pub agent_model: String,
    /// When the session started (epoch ms) — the ledger window opens here.
    pub since_ms: i64,
    /// The agent's own id for the current turn, once it has taken it.
    pub agent_run: Option<String>,
}

/// A turn whose last frame is older than this, with nothing running, reads as
/// quiet: the agent is mid-turn and saying nothing.
const QUIET_MS: i64 = 5 * 60_000;

/// The shared fleet key's ledger name (`fleet-brain`'s GATEWAY_KEY_NAME as
/// the gateway books it). Calls on it cannot be tied to an agent, let alone a
/// ticket: an agent container still on it predates per-agent keys.
const SHARED_KEY_CALLER: &str = "api:fleet-gateway";

/// What the session is doing, in one word, for a reader that has a second.
fn state_of(pulse: &Value, accepted: bool, now: i64) -> &'static str {
    if !pulse["step"].is_null() {
        return "running";
    }
    let last = pulse["lastAt"].as_i64();
    let turn_at = pulse["turnAt"].as_i64();
    let since_turn = matches!((last, turn_at), (Some(l), Some(t)) if l >= t);
    if !accepted && !since_turn {
        // Sent, and the agent has not named a run for it: booting, at its
        // concurrent-run cap, or unreachable. Not working yet.
        return "sending";
    }
    match last.or(turn_at) {
        Some(at) if now - at > QUIET_MS => "quiet",
        _ => "thinking",
    }
}

#[derive(Default, Clone)]
struct Spend {
    calls: i64,
    tokens: i64,
    cost: Option<f64>,
    unpriced: i64,
}

async fn spend_of(pg: &PgPool, live: &[Live]) -> HashMap<String, Spend> {
    let tasks: Vec<String> = live.iter().map(|l| l.task_id.clone()).collect();
    let since: Vec<i64> = live.iter().map(|l| l.since_ms).collect();
    let priced = talaria_api_facades::gateway::usage::priced_view();
    let sql = format!(
        "with s as (select * from unnest($1::uuid[], $2::bigint[]) as s(task_id, since_ms)), \
              priced as ({priced}) \
         select s.task_id::text, count(p.id)::bigint, \
                coalesce(sum(p.prompt_tokens + p.completion_tokens + p.cache_write_tokens \
                             + p.cache_read_tokens), 0)::bigint, \
                sum(p.cost)::float8, \
                (count(p.id) filter (where p.cost is null))::bigint \
         from s left join priced p \
           on p.task_id = s.task_id and p.created_at >= to_timestamp(s.since_ms / 1000.0) \
         group by s.task_id"
    );
    // AssertSqlSafe: the only interpolation is the gateway's priced view, a
    // constant; every value is a bind.
    let rows: Vec<(String, i64, i64, Option<f64>, i64)> =
        match sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(&tasks)
            .bind(&since)
            .fetch_all(pg)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[work-status] spend read failed: {e}");
                return HashMap::new();
            }
        };
    rows.into_iter()
        .map(|(task, calls, tokens, cost, unpriced)| {
            (
                task,
                Spend {
                    calls,
                    tokens,
                    cost: if calls > 0 { cost } else { None },
                    unpriced,
                },
            )
        })
        .collect()
}

/// Calls in the window the ledger could not tie to any ticket: the agent's
/// own `unattended` rows (its Hermes did not send the session key — Hermes
/// before v0.22 ignores `session_affinity_header`), and rows on the shared
/// fleet key (a container that has not been rolled onto its own key).
async fn untagged_of(pg: &PgPool, models: &[String], since_ms: i64) -> (HashMap<String, i64>, i64) {
    let mut by_agent = HashMap::new();
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "select agent_model, count(*)::bigint from usage_events \
         where agent_model = any($1) and source = 'unattended' \
           and created_at >= to_timestamp($2 / 1000.0) \
         group by agent_model",
    )
    .bind(models)
    .bind(since_ms)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    for (m, n) in rows {
        by_agent.insert(m, n);
    }
    let shared: i64 = sqlx::query_scalar(
        "select count(*)::bigint from usage_events \
         where agent_model = $1 and created_at >= to_timestamp($2 / 1000.0)",
    )
    .bind(SHARED_KEY_CALLER)
    .bind(since_ms)
    .fetch_one(pg)
    .await
    .unwrap_or(0);
    (by_agent, shared)
}

/// The status of each live session, in `live`'s order: `{ activity, spend }`
/// to merge into the session's wire object.
pub async fn statuses(state: &AppState, live: &[Live]) -> Vec<Value> {
    if live.is_empty() {
        return Vec::new();
    }
    let now = talaria_work_pulse::now_ms();
    let ids: Vec<String> = live.iter().map(|l| l.run_id.clone()).collect();
    let pulses = match state.redis().await {
        Ok(mut conn) => talaria_work_pulse::read_many(&mut conn, &ids)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("[work-status] pulse read failed: {e}");
                Vec::new()
            }),
        Err(_) => Vec::new(),
    };
    let spend = spend_of(&state.pg, live).await;
    let mut models: Vec<String> = live.iter().map(|l| l.agent_model.clone()).collect();
    models.sort();
    models.dedup();
    let since = live.iter().map(|l| l.since_ms).min().unwrap_or(now);
    let (untagged, shared) = untagged_of(&state.pg, &models, since).await;

    live.iter()
        .enumerate()
        .map(|(i, l)| {
            let mut pulse = match pulses.get(i) {
                Some((p, turn)) => talaria_work_pulse::wire(p, *turn),
                None => talaria_work_pulse::wire(&Default::default(), None),
            };
            let accepted = l.agent_run.is_some();
            pulse["accepted"] = json!(accepted);
            pulse["state"] = json!(state_of(&pulse, accepted, now));
            let s = spend.get(&l.task_id).cloned().unwrap_or_default();
            let agent_untagged = untagged.get(&l.agent_model).copied().unwrap_or(0);
            // Only a session with nothing on its ticket needs the reason: one
            // that has rows is metered, whatever else the agent is doing.
            let missing = if s.calls > 0 {
                Value::Null
            } else if agent_untagged > 0 {
                json!({ "calls": agent_untagged, "reason": "untagged" })
            } else if shared > 0 {
                json!({ "calls": shared, "reason": "shared-key" })
            } else {
                Value::Null
            };
            json!({
                "activity": pulse,
                "spend": {
                    "calls": s.calls,
                    "tokens": s.tokens,
                    "cost": s.cost,
                    "unpriced": s.unpriced,
                    "missing": missing,
                },
                "now": now,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pulse(step: bool, last: Option<i64>, turn_at: Option<i64>) -> Value {
        json!({
            "step": if step { json!({"tool": "terminal"}) } else { Value::Null },
            "lastAt": last,
            "turnAt": turn_at,
        })
    }

    #[test]
    fn a_session_reads_as_what_it_is_doing() {
        let now = 10_000_000;
        assert_eq!(
            state_of(&pulse(true, Some(now - 3_600_000), Some(0)), true, now),
            "running"
        );
        // Sent, not taken, nothing on the tail since the turn began.
        assert_eq!(
            state_of(&pulse(false, None, Some(now - 60_000)), false, now),
            "sending"
        );
        assert_eq!(
            state_of(
                &pulse(false, Some(now - 120_000), Some(now - 60_000)),
                false,
                now
            ),
            "sending"
        );
        // Frames since the turn began: the agent took it, whatever the row says.
        assert_eq!(
            state_of(
                &pulse(false, Some(now - 1_000), Some(now - 60_000)),
                false,
                now
            ),
            "thinking"
        );
        assert_eq!(
            state_of(&pulse(false, Some(now - QUIET_MS - 1), Some(0)), true, now),
            "quiet"
        );
    }
}
