// Proactive outreach (#59) — agents notice things and reach out instead of
// waiting to be asked. The sweep half: each opted-in agent gets a periodic
// "check-in turn" through its OWN persona gateway. The agent acts through its
// normal MCP tools (comment / post_to_channel / message_user), so everything
// stays attributed, board-policy-gated, and guard-visible; Talaria only
// delivers the nudge + signals.
//
// Port of outreach.ts, REDUCED to the scheduled half. `agentMessageUser` is
// the workbench MCP tool's write path and `recentOutreachEvents` is the admin
// view's read; both cross with their callers (the MCP server and the admin
// console, later batches) — the sweep is the piece the scheduler owns, and
// `outreach_events` — the memory that powers the caps, the "don't repeat
// yourself" context, and admin visibility — is written here.
//
// Everything is opt-in twice over: a master switch (off by default) AND a
// per-agent `proactive` flag.

use std::sync::Arc;

use crate::gateway::settings::get_setting;
use crate::harness::defs::outreach::{
    NOTHING_TO_SURFACE, OutreachCheckInInput, OutreachNote, OutreachTicket,
    outreach_check_in_harness,
};
use crate::harness::run::{RunContext, run_harness};
use crate::scheduler::{JobName, JobSpec};
use crate::state::AppState;
use serde_json::{Value, json};
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq)]
pub struct OutreachConfig {
    /// Master switch for the periodic sweep. `message_user` works regardless —
    /// it's a governed tool, not a behavior (it crosses with its caller).
    pub enabled: bool,
    /// Minimum minutes between check-in turns for any one agent.
    pub interval_minutes: i64,
    /// Agent-initiated DMs allowed per agent↔user pair per day.
    pub daily_dm_cap: i64,
}

const DEFAULT_CONFIG: OutreachConfig = OutreachConfig {
    enabled: false,
    interval_minutes: 240,
    daily_dm_cap: 3,
};

/// The stored partial over the defaults — TS's `{ ...DEFAULT_CONFIG, ...c }`.
/// A stored JSON null is falsy in TS and falls to the default field here;
/// nothing real writes one.
fn parse_config(stored: &Value) -> OutreachConfig {
    OutreachConfig {
        enabled: stored
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(DEFAULT_CONFIG.enabled),
        interval_minutes: stored
            .get("intervalMinutes")
            .and_then(|v| v.as_i64())
            .filter(|m| *m > 0)
            .unwrap_or(DEFAULT_CONFIG.interval_minutes),
        daily_dm_cap: stored
            .get("dailyDmCap")
            .and_then(|v| v.as_i64())
            .filter(|c| *c > 0)
            .unwrap_or(DEFAULT_CONFIG.daily_dm_cap),
    }
}

/// setOutreachConfig — a full-object write of the three knobs.
pub async fn set_outreach_config(pg: &PgPool, c: &OutreachConfig) {
    let _ = crate::gateway::settings::set_setting(
        pg,
        "outreach_config",
        &json!({
            "enabled": c.enabled,
            "intervalMinutes": c.interval_minutes,
            "dailyDmCap": c.daily_dm_cap,
        }),
    )
    .await;
}

/// The sweep's recent activity (outreach.ts recentOutreachEvents), newest
/// first — what an admin sees when they ask "is this thing doing anything".
pub async fn recent_outreach_events(pg: &PgPool, limit: i64) -> Vec<serde_json::Value> {
    #[allow(clippy::type_complexity)] // the select's four columns, in order
    let rows: Result<Vec<(String, String, Option<String>, i64)>, _> = sqlx::query_as(
        "select agent_model, kind, note, (trunc(extract(epoch from created_at) * 1000))::bigint \
         from outreach_events order by created_at desc limit $1",
    )
    .bind(limit)
    .fetch_all(pg)
    .await;
    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(|(agent_model, kind, note, created_ms)| {
                json!({
                    "agentModel": agent_model,
                    "kind": kind,
                    "note": note,
                    "createdAt": crate::agent_auth::epoch_ms_to_iso(created_ms),
                })
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

pub async fn get_outreach_config(pg: &PgPool) -> OutreachConfig {
    parse_config(&get_setting(pg, "outreach_config", json!({})).await)
}

async fn log_sweep_event(pg: &PgPool, agent_model: &str, note: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into outreach_events (agent_model, kind, user_id, conversation_id, note) \
         values ($1, 'sweep', null, null, $2)",
    )
    .bind(agent_model)
    .bind(note)
    .execute(pg)
    .await?;
    Ok(())
}

// ── the sweep ────────────────────────────────────────────────────────────────

const SWEEP_TICK_MS: u64 = 5 * 60_000; // how often a sweep is attempted

// THE DEPLOY-DAY BOUND. "Due" is computed from outreach_events, so an agent
// that has never been swept is due — the first pass on a real schedule
// therefore finds EVERY proactive agent due at once, and each due agent gets a
// check-in turn that may DM a human. Capping the pass turns that wall into a
// queue: at most this many agents per pass, one pass per 5 minutes, so a
// fleet of thirty proactive agents drains over ~50 minutes instead of
// messaging everyone in one burst. Agents keep their place — the query orders
// by slug and the uncapped remainder is simply still due next pass. (The
// master switch is off by default, so this only bites deployments that have
// deliberately turned outreach on.)
const SWEEP_MAX_AGENTS: usize = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepResult {
    pub turns: usize,
    pub failed: usize,
    pub deferred: usize,
}

/// One check-in turn per due agent, at most SWEEP_MAX_AGENTS per pass. "Due" =
/// proactive + enabled + no sweep event within the configured interval.
/// Serial on purpose — one agent turn at a time keeps the load invisible.
pub async fn sweep_outreach(deps: &OutreachDeps) -> Result<SweepResult, String> {
    let cfg = get_outreach_config(&deps.state.pg).await;
    if !cfg.enabled {
        return Ok(SweepResult::default());
    }
    // The row carries display_name and owner_user_id for the DM path
    // (`agentMessageUser`, which crosses with its caller); the TURN needs only
    // the model, so only the model is read here.
    let due: Vec<(String,)> = sqlx::query_as(
        "select d.model from agent_defs d \
         where d.enabled and d.proactive \
           and not exists ( \
             select 1 from outreach_events e \
             where e.agent_model = d.model and e.kind = 'sweep' \
               and e.created_at > now() - make_interval(mins => $1) \
           ) \
         order by d.slug limit $2",
    )
    .bind(cfg.interval_minutes as i32)
    .bind((SWEEP_MAX_AGENTS + 1) as i64)
    .fetch_all(&deps.state.pg)
    .await
    .map_err(|e| format!("due-agent query failed: {e}"))?;
    // One extra row is fetched purely to report the backlog honestly.
    let deferred = due.len().saturating_sub(SWEEP_MAX_AGENTS);

    let mut result = SweepResult {
        deferred,
        ..Default::default()
    };
    for (model,) in due.into_iter().take(SWEEP_MAX_AGENTS) {
        // Claim the slot BEFORE the turn — a crash mid-turn must not mean a
        // retry storm on the next pass.
        log_sweep_event(&deps.state.pg, &model, "(started)")
            .await
            .map_err(|e| format!("sweep claim failed: {e}"))?;
        // The note lands in outreach_events for the admin view, but an agent
        // whose check-in fails every pass should also be visible in the log.
        let note = match check_in_turn(&deps.state, &model).await {
            Ok(note) => note,
            Err(e) => {
                result.failed += 1;
                tracing::error!("[outreach] check-in turn for {model} failed: {e}");
                format!("error: {e}")
            }
        };
        result.turns += 1;
        sqlx::query(
            "update outreach_events set note = $2 where id = ( \
               select id from outreach_events \
               where agent_model = $1 and kind = 'sweep' \
               order by created_at desc limit 1 \
             )",
        )
        .bind(&model)
        .bind(note.chars().take(500).collect::<String>())
        .execute(&deps.state.pg)
        .await
        .map_err(|e| format!("sweep note write failed: {e}"))?;
    }
    Ok(result)
}

/// The signals + rules for one agent's check-in, sent through its own persona
/// gateway so any action it takes uses its normal, governed MCP tools.
async fn check_in_turn(state: &AppState, model: &str) -> Result<String, String> {
    // Signals: the agent's own assigned work with staleness, anything it sent
    // to quality review that's still waiting, and its recent outreach (so it
    // doesn't repeat itself).
    let work: Vec<(String, String, String, String, i32)> = sqlx::query_as(
        "select t.id::text, t.title, t.status, b.name, \
                extract(epoch from now() - t.updated_at)::int / 3600 \
         from tasks t join boards b on b.id = t.board_id \
         where t.assignees @> $1::jsonb \
           and ( \
             t.status = 'blocked' \
             or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id \
               and bs.category in ('open', 'active', 'review') and bs.key <> ( \
                 select bs2.key from board_statuses bs2 where bs2.board_id = t.board_id \
                   and bs2.category = 'open' and not bs2.agent_start order by bs2.position limit 1)) \
             or ( \
               not exists (select 1 from board_statuses bs where bs.board_id = t.board_id) \
               and t.status in ('assigned', 'in_progress', 'quality_review') \
             ) \
           ) \
           and t.archived_at is null \
         order by t.updated_at asc limit 15",
    )
    .bind(json!([model]))
    .fetch_all(&state.pg)
    .await
    .map_err(|e| format!("work signals query failed: {e}"))?;
    let work: Vec<OutreachTicket> = work
        .into_iter()
        .map(|(id, title, status, board, idle_hours)| OutreachTicket {
            id,
            title,
            status,
            board,
            idle_hours: idle_hours as f64,
        })
        .collect();

    let recent: Vec<(String, String)> = sqlx::query_as(
        "select kind, note from outreach_events \
         where agent_model = $1 and note is not null and note not in ('(started)', $2) \
           and created_at > now() - interval '7 days' \
         order by created_at desc limit 10",
    )
    .bind(model)
    .bind(NOTHING_TO_SURFACE)
    .fetch_all(&state.pg)
    .await
    .map_err(|e| format!("recent-outreach query failed: {e}"))?;
    let recent: Vec<OutreachNote> = recent
        .into_iter()
        .map(|(kind, note)| OutreachNote { kind, note })
        .collect();

    // The prompt, the rules block and the NOTHING_TO_SURFACE contract live in
    // the outreach harness; this function's job is the two signal queries
    // above. The model is the AGENT'S OWN — there is no platform-agent slot
    // for outreach and there should not be one, so the harness declares no
    // chain and takes the model from here (the pin, the only supported way
    // to bypass resolution).
    let input = serde_json::to_value(OutreachCheckInInput { work, recent })
        .map_err(|e| format!("check-in input encode failed: {e}"))?;
    let run = run_harness(
        state,
        &outreach_check_in_harness(),
        &input,
        RunContext {
            caller: format!("outreach:{model}"),
            model: Some(model.to_string()),
            ..RunContext::default()
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    // A silent turn is not a failure — the harness's declared fallback IS the
    // NOTHING token. A null value means the turn never completed (the gateway
    // was down, the stream died), which is what the sweep records as
    // `error: …` on the event row.
    run.value
        .and_then(|v| v.as_str().map(str::to_string))
        .ok_or_else(|| {
            run.error
                .clone()
                .unwrap_or_else(|| "the check-in turn produced no reply".into())
        })
}

pub struct OutreachDeps {
    pub state: AppState,
}

// ── The registration ────────────────────────────────────────────────────────

/// The job's sentence. A pass with no turns is `None` — an empty due list (or
/// the switch off) is a quiet pass, not a report.
fn sweep_message(r: &SweepResult) -> Option<String> {
    if r.turns == 0 {
        return None;
    }
    let mut line = format!("{} check-in turn(s)", r.turns);
    if r.failed > 0 {
        line.push_str(&format!(", {} failed", r.failed));
    }
    if r.deferred > 0 {
        line.push_str(&format!(
            ", {}+ agent(s) still due (capped at {SWEEP_MAX_AGENTS}/pass)",
            r.deferred
        ));
    }
    Some(line)
}

/// The job the scheduler runs. NOT `per_instance`: the sweep's input is the
/// `agent_defs` table and its memory is `outreach_events` — both shared — and
/// two instances sweeping would DM the same humans twice, the exact harm the
/// lease exists to prevent.
pub fn outreach_job_spec(deps: Arc<OutreachDeps>) -> JobSpec {
    JobSpec {
        name: JobName::OutreachSweep,
        // Unchanged: the same 5 minutes the kick throttled to.
        every_ms: SWEEP_TICK_MS,
        // Long enough that a deploy (and a crash loop) never reaches the point
        // of messaging a human, and that the fleet's agents are registered
        // first.
        first_run_delay_ms: Some(5 * 60_000),
        // SWEEP_MAX_AGENTS agent turns through the gateway, serially.
        max_run_ms: Some(10 * 60_000),
        per_instance: false,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move {
                let r = sweep_outreach(&deps).await?;
                Ok(sweep_message(&r))
            })
        }),
    }
}

/// Declare the sweep to the scheduler — the function the flip calls from
/// boot. 'outreach-sweep' is in REQUIRED_JOBS, so an instance that boots
/// without reaching it prints a MISSING JOBS error instead of quietly never
/// checking in.
pub fn register_outreach_job(deps: Arc<OutreachDeps>) {
    crate::scheduler::register_job(outreach_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;

    // The TS module has no test file of its own; these pin the pure halves
    // the sweep stands on. The queries and the harness turn are exercised by
    // the flip's wiring, never by CI (no service-dependent tests — house
    // rule).

    #[test]
    fn an_empty_stored_config_is_all_defaults() {
        assert_eq!(parse_config(&json!({})), DEFAULT_CONFIG);
    }

    #[test]
    fn a_partial_stored_config_keeps_the_defaults_for_absent_keys() {
        let c = parse_config(&json!({ "enabled": true }));
        assert!(c.enabled);
        assert_eq!(c.interval_minutes, 240);
        assert_eq!(c.daily_dm_cap, 3);
    }

    #[test]
    fn a_full_stored_config_is_honored() {
        let c = parse_config(&json!({ "enabled": true, "intervalMinutes": 30, "dailyDmCap": 1 }));
        assert_eq!(
            c,
            OutreachConfig {
                enabled: true,
                interval_minutes: 30,
                daily_dm_cap: 1
            }
        );
    }

    #[test]
    fn non_positive_or_malformed_numbers_fall_back_rather_than_half_fire() {
        // `intervalMinutes: 0` or a string would make every agent "due" on
        // every tick in TS (the interval math degrades); here it falls to the
        // default. A quieter failure mode than TS's, and only reachable by a
        // hand-edited row.
        let c = parse_config(&json!({ "intervalMinutes": 0, "dailyDmCap": "three" }));
        assert_eq!(c.interval_minutes, 240);
        assert_eq!(c.daily_dm_cap, 3);
    }

    #[test]
    fn a_stored_null_is_falsy_like_ts() {
        let c = parse_config(&json!({ "enabled": null }));
        assert!(!c.enabled);
    }

    #[test]
    fn a_pass_with_no_turns_is_nothing_to_do() {
        assert_eq!(sweep_message(&SweepResult::default()), None);
    }

    #[test]
    fn a_landed_pass_names_its_failures_and_its_backlog() {
        let r = SweepResult {
            turns: 3,
            failed: 1,
            deferred: 2,
        };
        assert_eq!(
            sweep_message(&r),
            Some("3 check-in turn(s), 1 failed, 2+ agent(s) still due (capped at 3/pass)".into())
        );
    }

    #[test]
    fn a_clean_pass_reports_just_the_turns() {
        let r = SweepResult {
            turns: 1,
            deferred: 0,
            failed: 0,
        };
        assert_eq!(sweep_message(&r), Some("1 check-in turn(s)".into()));
    }
}
