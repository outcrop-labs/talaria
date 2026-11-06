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
//     ticket says so and why, rather than reading as free — and only when
//     the reason is THIS agent's, in THIS session's window.
//
// The run's own state comes first: a queued run is queued (whatever its
// previous turn left on the tail), one awaiting a person is waiting on a
// person, and a pulse that could not be read is "unknown", never a guess.

use std::collections::HashMap;

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_state::AppState;

/// One live session to report on.
pub struct Live {
    pub run_id: String,
    pub task_id: String,
    pub agent_model: String,
    /// The run row's own state: `queued`, `running` or `awaiting`.
    pub run_state: String,
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
/// The run's own state decides first: a `queued` run has no turn in flight
/// (it is waiting to be driven — a fresh session, or one re-queued after an
/// api restart), and an `awaiting` one is parked on a person.
fn state_of(pulse: &Value, accepted: bool, now: i64, run_state: &str) -> &'static str {
    match run_state {
        "queued" => return "queued",
        "awaiting" => return "awaiting",
        _ => {}
    }
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

/// Why a session with nothing on its ticket has nothing there — or `None`
/// when there is nothing honest to say. Pure.
///
/// - `untagged`: THIS agent made calls in THIS session's window that carry no
///   session key — booked under the agent, `unattended`, with no affinity
///   key (a Hermes cron's calls carry `cron_<id>` and are not counted). A
///   Hermes before v0.22 sends none: it ignores `session_affinity_header`.
/// - `shared-key`: the session has done work (frames on its tail), not one
///   call landed under this agent's own name in its window, and calls did
///   arrive on the fleet's shared key in that window. The agent's turns call
///   the model, so its calls went somewhere; the shared key is the only
///   place an agent's calls are booked under another name.
/// - nothing while the turn is not running yet (queued, sending, awaiting,
///   unknown): "no model calls yet" is the true sentence then.
fn missing_reason(state: &str, worked: bool, w: &Window) -> Value {
    if w.on_ticket > 0 || matches!(state, "queued" | "sending" | "awaiting" | "unknown") {
        return Value::Null;
    }
    if w.untagged > 0 {
        return json!({ "calls": w.untagged, "reason": "untagged" });
    }
    if worked && w.own == 0 && w.shared > 0 {
        return json!({ "calls": w.shared, "reason": "shared-key" });
    }
    Value::Null
}

/// One session's ledger window, counted from its own start.
#[derive(Default, Clone, Copy)]
struct Window {
    /// Calls booked to the ticket.
    on_ticket: i64,
    /// This agent's calls with no session key (crons excluded).
    untagged: i64,
    /// Every call booked under this agent's own name.
    own: i64,
    /// Every call on the shared fleet key.
    shared: i64,
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

/// Each session's [`Window`] counts, in `live`'s order, one statement.
async fn windows_of(pg: &PgPool, live: &[Live]) -> Vec<Window> {
    let models: Vec<String> = live.iter().map(|l| l.agent_model.clone()).collect();
    let since: Vec<i64> = live.iter().map(|l| l.since_ms).collect();
    let rows: Vec<(i64, i64, i64, i64)> = sqlx::query_as(
        "select s.i, \
                (select count(*) from usage_events u \
                  where u.agent_model = s.model and u.created_at >= to_timestamp(s.since_ms / 1000.0) \
                    and u.source = 'unattended' and u.affinity_key is null)::bigint, \
                (select count(*) from usage_events u \
                  where u.agent_model = s.model and u.created_at >= to_timestamp(s.since_ms / 1000.0))::bigint, \
                (select count(*) from usage_events u \
                  where u.agent_model = $3 and u.created_at >= to_timestamp(s.since_ms / 1000.0))::bigint \
         from unnest($1::text[], $2::bigint[]) with ordinality as s(model, since_ms, i) \
         order by s.i",
    )
    .bind(&models)
    .bind(&since)
    .bind(SHARED_KEY_CALLER)
    .fetch_all(pg)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("[work-status] attribution read failed: {e}");
        Vec::new()
    });
    let mut out = vec![Window::default(); live.len()];
    for (i, untagged, own, shared) in rows {
        if let Some(w) = usize::try_from(i - 1).ok().and_then(|i| out.get_mut(i)) {
            *w = Window {
                on_ticket: 0,
                untagged,
                own,
                shared,
            };
        }
    }
    out
}

/// The status of each live session, in `live`'s order: `{ activity, spend }`
/// to merge into the session's wire object.
pub async fn statuses(state: &AppState, live: &[Live]) -> Vec<Value> {
    if live.is_empty() {
        return Vec::new();
    }
    let now = talaria_work_pulse::now_ms();
    let ids: Vec<String> = live.iter().map(|l| l.run_id.clone()).collect();
    // None: the pulse could not be read. Every card then says "unknown"
    // rather than a state made up from an empty tail.
    let pulses = match state.redis().await {
        Ok(mut conn) => match talaria_work_pulse::read_many(&mut conn, &ids).await {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("[work-status] pulse read failed: {e}");
                None
            }
        },
        Err(e) => {
            tracing::warn!("[work-status] redis unreachable: {e}");
            None
        }
    };
    let spend = spend_of(&state.pg, live).await;
    let windows = windows_of(&state.pg, live).await;

    live.iter()
        .enumerate()
        .map(|(i, l)| {
            let accepted = l.agent_run.is_some();
            let (mut pulse, word) = match pulses.as_ref().and_then(|p| p.get(i)) {
                Some((p, turn)) => {
                    let mut wire = talaria_work_pulse::wire(p, *turn);
                    let word = state_of(&wire, accepted, now, &l.run_state);
                    if l.run_state != "running" {
                        // No turn is in flight: whatever the tail says is
                        // running belongs to a turn that ended (an api
                        // restart mid-step re-queues the run), so no step.
                        wire["step"] = Value::Null;
                        wire["outer"] = Value::Null;
                    }
                    (wire, word)
                }
                None => (
                    talaria_work_pulse::wire(&Default::default(), None),
                    "unknown",
                ),
            };
            pulse["accepted"] = json!(accepted);
            pulse["state"] = json!(word);
            let s = spend.get(&l.task_id).cloned().unwrap_or_default();
            let mut w = windows.get(i).copied().unwrap_or_default();
            w.on_ticket = s.calls;
            let worked = pulse["lastAt"].is_i64() || pulse["steps"].as_u64().unwrap_or(0) > 0;
            json!({
                "activity": pulse,
                "spend": {
                    "calls": s.calls,
                    "tokens": s.tokens,
                    "cost": s.cost,
                    "unpriced": s.unpriced,
                    "missing": missing_reason(word, worked, &w),
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
        let run = |p: &Value, accepted: bool| state_of(p, accepted, now, "running");
        assert_eq!(
            run(&pulse(true, Some(now - 3_600_000), Some(0)), true),
            "running"
        );
        // Sent, not taken, nothing on the tail since the turn began.
        assert_eq!(
            run(&pulse(false, None, Some(now - 60_000)), false),
            "sending"
        );
        assert_eq!(
            run(
                &pulse(false, Some(now - 120_000), Some(now - 60_000)),
                false
            ),
            "sending"
        );
        // Frames since the turn began: the agent took it, whatever the row says.
        assert_eq!(
            run(&pulse(false, Some(now - 1_000), Some(now - 60_000)), false),
            "thinking"
        );
        assert_eq!(
            run(&pulse(false, Some(now - QUIET_MS - 1), Some(0)), true),
            "quiet"
        );
    }

    #[test]
    fn the_run_row_decides_before_the_tail() {
        let now = 10_000_000;
        // A fresh queued run: no turn key, no agent run — not "sent".
        assert_eq!(
            state_of(&pulse(false, None, None), false, now, "queued"),
            "queued"
        );
        // Re-queued after an api restart mid-step: the dead turn's open
        // terminal frame is still on the tail. Still queued, not running.
        assert_eq!(
            state_of(
                &pulse(true, Some(now - 1_000), Some(now - 9_000)),
                true,
                now,
                "queued"
            ),
            "queued"
        );
        // Parked on a person for hours: waiting, not "quiet 2h".
        assert_eq!(
            state_of(
                &pulse(false, Some(now - 7_200_000), Some(0)),
                true,
                now,
                "awaiting"
            ),
            "awaiting"
        );
    }

    #[test]
    fn a_missing_reason_is_this_agents_and_only_when_true() {
        let w = |untagged, own, shared| Window {
            on_ticket: 0,
            untagged,
            own,
            shared,
        };
        // Booked to the ticket: nothing to explain.
        assert!(
            missing_reason(
                "running",
                true,
                &Window {
                    on_ticket: 3,
                    ..w(5, 5, 5)
                }
            )
            .is_null()
        );
        // This agent's own calls carried no session key.
        assert_eq!(
            missing_reason("running", true, &w(4, 4, 0))["reason"],
            "untagged"
        );
        // Shared-key calls happened, but this agent's own name has calls too:
        // it is on its own key, so the shared ones are someone else's.
        assert!(missing_reason("thinking", true, &w(0, 7, 12)).is_null());
        // Nothing under its own name while it worked, and shared-key calls in
        // its window: its calls went there.
        assert_eq!(
            missing_reason("running", true, &w(0, 0, 12))["reason"],
            "shared-key"
        );
        // ...but not for a session that has done nothing yet.
        assert!(missing_reason("thinking", false, &w(0, 0, 12)).is_null());
        // A turn not running yet has made no calls: say nothing.
        for state in ["queued", "sending", "awaiting", "unknown"] {
            assert!(
                missing_reason(state, true, &w(4, 0, 12)).is_null(),
                "{state}"
            );
        }
    }
}
