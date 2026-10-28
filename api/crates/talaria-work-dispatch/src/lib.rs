// Work dispatch — THE PUSH SIDE. When a ticket enters an agent-start column
// with agent assignees, Talaria pushes the work into the agent's own persona
// gateway as a WORK SESSION: turn after turn, the agent drives its
// tools/harness like a developer at a desk (run, read, steer, test, repeat),
// until the ticket reaches review/blocked/done, the agent stops progressing,
// or the turn cap lands. One turn is never the budget for real work. Matched
// task WORKFLOWS ride along; the sixty-second redispatch sweep below is the
// safety net for a push that never landed. A workchain step whose
// predecessor is still live is not dispatched at all (see the gate in
// `maybe_dispatch_ticket`).
//
// THE DRIVE POSTURE. This process enqueues the session AND drives it —
// insert, publish, detached drive, all here. The shared-table shape is what
// makes that safe: the runs table's DERIVED id means two dispatchers racing
// one ticket still compute the same row id for the same ticket+agent, so
// racing instances produce ONE session, never two.
//
// THE CLAIM:
//   `session_run_id(task, agent, n)` is a DERIVED uuid, so two dispatchers
//   racing the same ticket compute the same id and the primary key refuses
//   the second — across instances, which a process-local set never could.
//   The walk over `n` is what keeps that claim from being permanent:
//   generation 0 is this pair's first session ever, and once it is FINISHED a
//   ticket that legitimately comes back to the same agent needs somewhere to
//   put the next one. So:
//     · a live (or parked) run at generation n → stand down, that is the session
//     · a finished run at generation n → that session is over, look at n + 1
//     · nothing at generation n → claim it
//   A duplicate-key error on the claim is the race resolving itself: somebody
//   inserted between our read and our write, and what they inserted is a live
//   session for this exact ticket and agent, so standing down is correct.

use std::collections::HashMap;
use std::sync::Arc;

use sqlx::PgPool;
use talaria_agent_auth::now_ms as wall_ms;
use talaria_runs_define::{RunState, is_terminal};
use talaria_runs_run::{EnqueueOptions, RunDeps, enqueue};
use talaria_runs_work_session::{INTERRUPTED_TURN_SETTLE_MS, session_run_id, work_session_run};
use talaria_statuses as statuses;
use talaria_tasks as tasks;
use talaria_tasks_types::DispatchTicket;

const LOG: &str = "[work-dispatch]";

/// How many sessions one ticket+agent pair may ever have.
///
/// Not a rate limit — it is the bound on the generation walk below, and it is
/// a SIGNAL as much as a guard: a ticket that has been handed to the same
/// agent twenty-five times is a ticket bouncing between a column and an agent
/// that cannot finish it, and the honest response is to say so loudly and
/// stop rather than to quietly start a twenty-sixth.
pub const MAX_SESSIONS_PER_TICKET_AGENT: u32 = 25;

// Workbench dispatch no longer uses a per-agent job cap. The host's
// MemAvailable (minus a platform keep-back) is the queue — see
// `fleet::budget`. Conversation-only agents stay uncapped.

/// A primary-key collision, and the only error `enqueue` can raise that means
/// something rather than being broken: somebody else claimed this exact
/// session in the time between our read and our insert. SQLSTATE 23505 is
/// `unique_violation`. Same spelling as password_accounts' login collision.
fn is_duplicate_key(e: &sqlx::Error) -> bool {
    e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23505")
}

/// The dispatch assembly: the FULL real one, `real_run_deps`. A run that
/// parks a question actually parks, because the driver that will park it is
/// this process.
pub fn dispatch_deps(
    pg: PgPool,
    redis: redis::aio::ConnectionManager,
    rt: talaria_realtime::RealtimeDeps,
) -> RunDeps {
    talaria_runs_decide::assembly::real_run_deps(pg, redis, rt)
}

/// Drive one ticket with one agent as a SESSION. Fire-and-forget from task
/// mutations; re-entry for the same ticket+agent is a no-op.
///
/// IT DOES NOT WAIT FOR THE WORK. The session is a row before this function
/// returns and a driver picks it up detached — this process's scheduler. The
/// callers fire-and-forget; the session outlives them.
pub async fn dispatch_ticket_work(
    deps: &RunDeps,
    task: &DispatchTicket,
    agent_model: &str,
    board_name: Option<&str>,
) -> bool {
    let def = work_session_run();
    for generation in 0..MAX_SESSIONS_PER_TICKET_AGENT {
        let id = session_run_id(&task.id, agent_model, generation);
        let existing = match deps.store.get(&id).await {
            Ok(row) => row,
            Err(e) => {
                // The claim is the only thing standing between a retried
                // request and two agents on one ticket, so a store we cannot
                // read is a dispatch we do not make. The redispatch sweep
                // offers the ticket again within a minute.
                tracing::error!(
                    "{LOG} {}: could not check for a live session with {agent_model}, not dispatching: {e}",
                    task.id
                );
                return false;
            }
        };
        match existing {
            // A live (or parked) run at this generation IS the session.
            Some(row) if !is_terminal(row.state) => return true,
            // ERRED JUST NOW: the step that failed may still be running —
            // an idle deadline fires on SILENCE, and the persona's tool loop
            // can be mid-build with the stream gone. The next generation
            // waits the same settle a reclaimed turn gets before its next
            // prompt; the sweep re-offers the ticket every minute.
            Some(row)
                if row.state == RunState::Error
                    && talaria_agent_auth::iso_to_epoch_ms(&row.updated_at)
                        .is_some_and(|at| wall_ms() - at < INTERRUPTED_TURN_SETTLE_MS) =>
            {
                tracing::debug!(
                    "{LOG} {}: the last session with {agent_model} erred moments ago — letting \
                     its turn settle before a new one",
                    task.id
                );
                return false;
            }
            // Finished: that session is over, the next generation may start
            // a new one.
            Some(_) => continue,
            None => {}
        }
        let mut input = serde_json::json!({
            "taskId": task.id,
            "agentModel": agent_model,
            "boardId": task.board_id,
            "generation": generation,
        });
        // Only ever set by a caller that already had it; it goes into the
        // dispatch prompt's header and nowhere else.
        if let Some(name) = board_name {
            input["boardName"] = serde_json::Value::String(name.to_string());
        }
        let opts = EnqueueOptions {
            // NULL OWNER, deliberately. A work session belongs to a TICKET,
            // not to the person whose click happened to move the column —
            // `maybe_dispatch_ticket` is not told who acted, and inventing an
            // owner from the ticket's assignees would put somebody else's
            // agent session in a person's "my runs" strip. The subject is
            // the ticket and the audience is its board.
            owner_user_id: None,
            subject_type: Some("task".into()),
            subject_id: Some(task.id.clone()),
            phase: Some(format!("queued for {agent_model}")),
            id: Some(id),
            // This process drives what it enqueues — nothing else will.
            start: Some(true),
        };
        match enqueue(def, input, opts, deps).await {
            Ok(_) => return true,
            // Somebody else claimed this exact session between our read and
            // our insert — and what they inserted is a live session for this
            // ticket and agent, so standing down is correct.
            Err(e) if is_duplicate_key(&e) => return true,
            Err(e) => {
                tracing::error!(
                    "{LOG} {}: could not start a work session with {agent_model}: {e}",
                    task.id
                );
                return false;
            }
        }
    }
    tracing::error!(
        "{LOG} {} has had {MAX_SESSIONS_PER_TICKET_AGENT} work sessions with {agent_model} and \
         every one of them finished — not starting another. This ticket keeps coming back to an \
         agent that cannot close it; a person should look at it.",
        task.id
    );
    false
}

/// Dispatch to every AGENT assignee when the ticket sits in an agent-start
/// column. `only_agents` narrows to newly-added assignees on updates.
///
/// THE PUSH-SIDE CHOKE POINT. Every caller — task create, both branches of
/// task update, and anything added later — arrives here, and this checks ONE
/// thing about the column: is it somewhere work is actually picked up from?
/// `pickup_keys`, not `agent_start_keys`: the raw flag is the REFUSAL set
/// (entering any of those columns is assignment, which an agent may not do
/// for itself), while this is a DESTINATION question — a review column
/// carrying `agent_start` is not a pickup column, and dispatching into one
/// starts a session on a ticket the review-exit rule has already frozen. And
/// the agent gate is PER AGENT, because board policy is part of the question:
/// an agent still listed as an assignee on a board that has since revoked its
/// grant gets no fresh work session.
///
/// THE WORKCHAIN ORDER is the ticket's other question, asked once before any
/// agent is looked at: a step whose predecessor is still live is nobody's
/// work yet, so it starts no session and leaves no wait mark, whatever its
/// column says. `talaria_tasks::workchain_blocked` is the one derive.
///
/// IT IS STILL ASKED AGAIN INSIDE THE SESSION, on every turn. This gate is
/// about whether to START; a run may sit in the queue, park, or be reclaimed
/// after a deploy, and the first thing every step does is put the same
/// question to `agent_work_refusal` (the authority clauses plus the chain
/// order) again.
///
/// Fire-and-forget: nothing a dispatch failure can do may reach a caller.
/// Errors are logged here rather than swallowed silently.
pub async fn maybe_dispatch_ticket(
    pg: &PgPool,
    deps: &RunDeps,
    task: &DispatchTicket,
    only_agents: Option<&[String]>,
) {
    let meta = match statuses::status_meta(pg, &task.board_id).await {
        Ok(meta) => meta,
        Err(e) => {
            tracing::error!(
                "{LOG} {}: could not read the board's statuses, not dispatching: {e}",
                task.id
            );
            return;
        }
    };
    if !meta.pickup_keys.contains(&task.status) {
        return;
    }
    // THE WORKCHAIN ORDER, before any per-agent side effect (the RAM wait
    // mark below would otherwise tell a person a blocked step is queued for
    // memory). A chain we cannot read is a dispatch we do not make — the
    // sweep asks again in a minute.
    let metas = HashMap::from([(task.board_id.clone(), meta.clone())]);
    match tasks::workchain_blocked(pg, &metas, std::slice::from_ref(&task.id)).await {
        Ok(blocked) if blocked.contains(&task.id) => {
            tracing::debug!("{LOG} {}: {}", task.id, tasks::WORKCHAIN_WAIT);
            return;
        }
        Ok(_) => {}
        Err(e) => {
            tracing::error!(
                "{LOG} {}: could not read its workchain order, not dispatching: {e}",
                task.id
            );
            return;
        }
    }
    let target = tasks::AgentWriteTarget {
        board_id: task.board_id.clone(),
        status: task.status.clone(),
        archived_at: task.archived_at.clone(),
    };
    for agent in tasks::agent_assignees(&task.assignees) {
        if let Some(only) = only_agents
            && !only.contains(&agent)
        {
            continue;
        }
        // The concurrency cap, asked BEFORE the refusal walk — and scoped to
        // DEVELOPER agents only: their sessions spawn real processes in one
        // container, while a session for any other agent is just model
        // turns and stays uncapped. An agent already driving MAX_CONCURRENT
        // sessions gets no new one this pass; standing down quietly is the
        // design, because the sweep re-offers every sixty seconds — the skip
        // IS the queue. A def or count we cannot read is a dispatch we do not
        // make (same law as the claim read below).
        let def_row: Option<(bool, bool)> =
            match sqlx::query_as("select developer, enabled from agent_defs where model = $1")
                .bind(&agent)
                .fetch_optional(pg)
                .await
            {
                Ok(row) => row,
                Err(e) => {
                    tracing::error!(
                        "{LOG} {}: could not read {agent}'s def, not dispatching: {e}",
                        task.id
                    );
                    continue;
                }
            };
        // A DISABLED AGENT GETS NO SESSION. Its persona answers a canned
        // stream, the turn fails, and the sweep would restart it every five
        // minutes until the generation cap — twenty-five failed sessions for
        // a switch somebody turned off.
        if let Some((_, false)) = def_row {
            tracing::debug!("{LOG} {}: {agent} is disabled, not dispatching", task.id);
            continue;
        }
        // A PERSON STOPPED THIS PAIR. The stop button cancels the live
        // session and leaves the ticket where it is; without a durable mark
        // the sweep re-offered the same ticket to the same agent five
        // minutes later, and after every stop. The mark holds until a human
        // moves the ticket's status, which is the triage the stop asked for.
        match dispatch_block(pg, &task.id, &agent).await {
            Ok(None) => {}
            Ok(Some(by)) => {
                tracing::debug!(
                    "{LOG} {}: {agent} was stopped on this ticket by {by} — not dispatching \
                     until a person moves it",
                    task.id
                );
                continue;
            }
            Err(e) => {
                tracing::error!(
                    "{LOG} {}: could not read the stop mark for {agent}, not dispatching: {e}",
                    task.id
                );
                continue;
            }
        }
        let developer = def_row.map(|(developer, _)| developer);
        // No def row (an unmanaged name) is nothing this gate knows how to
        // classify; dispatch proceeds and the refusal walk below decides.
        let capped = match developer {
            Some(true) => {
                match talaria_fleet_budget::admit_work(talaria_fleet_budget::JOB_STANDARD).await {
                    Ok(()) => false,
                    Err(reason) => {
                        tracing::debug!(
                            "{LOG} {}: {agent} waiting on host RAM ({reason})",
                            task.id
                        );
                        talaria_work_wait::mark_waiting(pg, &task.id, &agent, &reason).await;
                        true
                    }
                }
            }
            Some(false) | None => false,
        };
        if capped {
            continue;
        }
        let subject = talaria_agent_auth::AgentSubject::Model(agent.clone());
        match tasks::agent_ticket_refusal(pg, &target, &subject, tasks::AgentIntent::Write).await {
            Ok(Some(_refusal)) => continue,
            Ok(None) => {
                if dispatch_ticket_work(deps, task, &agent, None).await {
                    talaria_work_wait::clear_waiting(pg, &task.id).await;
                }
            }
            Err(e) => {
                tracing::error!("{LOG} {}: dispatch to {agent} threw: {e}", task.id);
            }
        }
    }
}

/// The durable stop mark for one (ticket, agent), or `None` when there is
/// none — or when there WAS one and a person has moved the ticket's status
/// since, in which case the mark is cleared here and the pair is free again.
/// "A person" is an actor that is a user id: the agent's own status moves
/// (and Talaria's lines) do not lift a stop a human placed.
async fn dispatch_block(
    pg: &PgPool,
    task_id: &str,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    let block: Option<(String, bool)> = sqlx::query_as(
        "select b.blocked_by, exists ( \
           select 1 from task_activity a \
           where a.task_id = b.task_id and a.type = 'status' \
             and a.created_at > b.created_at \
             and a.actor ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$' \
         ) as moved \
         from work_dispatch_blocks b \
         where b.task_id = $1::uuid and b.agent_model = $2",
    )
    .bind(task_id)
    .bind(agent_model)
    .fetch_optional(pg)
    .await?;
    match block {
        None => Ok(None),
        Some((_, true)) => {
            sqlx::query(
                "delete from work_dispatch_blocks where task_id = $1::uuid and agent_model = $2",
            )
            .bind(task_id)
            .bind(agent_model)
            .execute(pg)
            .await?;
            Ok(None)
        }
        Some((by, false)) => Ok(Some(by)),
    }
}

// ── The self-heal sweep ──────────────────────────────────────────────────────
//
// Dispatch fires when a ticket ENTERS a pickup column, and only then — which
// made a dead session the end of the work: the session errored (a slow turn
// under an old wall-clock step ceiling, a deploy mid-drive, a driver that
// could not be reclaimed), the ticket stayed put, and nothing ever asked
// again. The 2026-09-14 incident was exactly this shape — turn 2 was
// mid-flight and demonstrably working when the step ceiling killed the
// session, and the ticket sat in_progress silent for seven hours.
//
// THE SWEEP is the missing pull side: every ticket that still sits in a
// pickup column with agent assignees is re-offered to
// `maybe_dispatch_ticket`, whose generation walk is the real gate — a live
// or parked session stands down, a finished one advances to the next
// generation, and the twenty-five cap says the loud sentence. All this sweep
// adds is the asking. A SETTLE window skips tickets whose latest session
// touched the runs table recently, so a session that just errored for a real
// reason gets quiet before a generation is spent on it — and a live session
// needs no protection from this filter at all, because the claim walk never
// advances past a live run.
pub const REDISPATCH_SETTLE_MS: i64 = 5 * 60_000;

/// What one sweep pass decided, per ticket. Only `offer` reaches
/// `maybe_dispatch_ticket`; the other two are reported, never offered.
#[derive(Debug, Default)]
pub struct SweepPass {
    /// Tickets in a pickup column with at least one agent assignee, not
    /// behind a live workchain step: re-offered to their agents.
    pub offer: Vec<DispatchTicket>,
    /// Ids of agent-assigned tickets sitting outside a pickup column.
    pub parked: Vec<String>,
    /// Ids of agent-assigned tickets waiting on an earlier workchain step.
    pub waiting: Vec<String>,
}

/// The pass in numbers — what the log line says, and what a test asserts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SweepCount {
    pub offered: usize,
    pub parked: usize,
    pub waiting: usize,
}

impl SweepPass {
    pub fn count(&self) -> SweepCount {
        SweepCount {
            offered: self.offer.len(),
            parked: self.parked.len(),
            waiting: self.waiting.len(),
        }
    }
}

/// The assignee list a sweep offers, or None when the ticket has nobody the
/// sweep can offer it to. AN AGENT ASSIGNEE, NOT ANY ASSIGNEE: a ticket
/// assigned only to people (`user:<id>`) gives `maybe_dispatch_ticket`
/// nothing to loop over, so offering it was a guaranteed no-op that the
/// count still called "re-offered", at the cost of a status and a workchain
/// read every minute. The full list rides on (people included):
/// `maybe_dispatch_ticket` filters it again, and is the one that decides.
fn sweep_assignees(assignees: Option<serde_json::Value>) -> Option<Vec<String>> {
    let assignees = tasks::json_strings(&assignees.filter(|v| !v.is_null())?);
    if tasks::agent_assignees(&assignees).is_empty() {
        return None;
    }
    Some(assignees)
}

/// Which tickets this pass offers, and which it reports as parked or
/// waiting. None when the pass cannot run (the sweep query or the
/// workchain read failed — logged here).
pub async fn sweep_pass(pg: &PgPool) -> Option<SweepPass> {
    // `assignees` is JSONB (see tasks.rs's own read): decoded as a Value and
    // mapped with the same json_strings law, not as TEXT[] — the trap the
    // first night of this sweep found the loud way.
    type Row = (
        String,
        String,
        String,
        Option<serde_json::Value>,
        Option<String>,
    );
    let rows: Vec<Row> = match sqlx::query_as(
        "select t.id::text, t.board_id::text, t.status, t.assignees, t.archived_at::text \
         from tasks t \
         where t.assignees <> '[]'::jsonb \
           and t.archived_at is null \
           and not exists ( \
             select 1 from runs r \
             where r.kind = 'work-session' and r.subject_id = t.id::text \
               and r.updated_at > now() - interval '5 minutes' \
           )",
    )
    .fetch_all(pg)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("{LOG} sweep query failed: {e}");
            return None;
        }
    };
    // THE COUNT IS THE DIAGNOSTIC. Offering a ticket whose column is not a
    // pickup column is a silent no-op inside maybe_dispatch_ticket — and a
    // sweep that counted those as "re-offered" read, during the 2026-09-22
    // incident, as "19 tickets moving" while zero were dispatchable and the
    // real question was why. Pickup keys are read once per board per pass
    // (the same status_meta the gate itself reads), and only tickets that
    // pass the column question here are offered and counted; everything
    // else is reported as parked, which is what it is. A ticket with no
    // agent assignee is neither: the sweep has nobody to offer it to, so it
    // is not counted at all (`sweep_assignees`).
    let mut meta_by_board: HashMap<String, Option<statuses::StatusMeta>> = HashMap::new();
    let mut in_pickup: Vec<DispatchTicket> = Vec::new();
    let mut pass = SweepPass::default();
    for (id, board_id, status, assignees, archived_at) in rows {
        let Some(assignees) = sweep_assignees(assignees) else {
            continue;
        };
        let meta = match meta_by_board.entry(board_id.clone()) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let meta = match statuses::status_meta(pg, &board_id).await {
                    Ok(meta) => Some(meta),
                    // A board whose statuses cannot be read is a board whose
                    // tickets are not dispatched this pass — the same law as
                    // the gate's own read, said here instead of unsaid.
                    Err(e) => {
                        tracing::error!(
                            "{LOG} {}: could not read the board's statuses, not offering: {e}",
                            id
                        );
                        None
                    }
                };
                v.insert(meta)
            }
        };
        if !meta
            .as_ref()
            .is_some_and(|m| m.pickup_keys.contains(&status))
        {
            pass.parked.push(id);
            continue;
        }
        in_pickup.push(DispatchTicket {
            id,
            board_id,
            status,
            assignees,
            archived_at,
        });
    }
    // THE WORKCHAIN ORDER, asked once for the whole pass: a step behind a
    // live predecessor is waiting its turn, not stuck, and counting it as
    // re-offered would be the 2026-09-22 lie again. maybe_dispatch_ticket
    // asks the same question per ticket anyway; this batch is what keeps
    // the count honest. A chain we cannot read stands the pass down.
    let metas: HashMap<String, statuses::StatusMeta> = meta_by_board
        .into_iter()
        .filter_map(|(board, meta)| meta.map(|m| (board, m)))
        .collect();
    let ids: Vec<String> = in_pickup.iter().map(|t| t.id.clone()).collect();
    let blocked = match tasks::workchain_blocked(pg, &metas, &ids).await {
        Ok(blocked) => blocked,
        Err(e) => {
            tracing::error!("{LOG} sweep could not read workchain order, not offering: {e}");
            return None;
        }
    };
    for task in in_pickup {
        if blocked.contains(&task.id) {
            pass.waiting.push(task.id);
        } else {
            pass.offer.push(task);
        }
    }
    Some(pass)
}

/// One sweep pass. Fire-and-forget by nature (a scheduler job calls it);
/// every failure is logged here and never thrown. Answers what it did.
pub async fn redispatch_agent_work(pg: &PgPool, deps: &RunDeps) -> SweepCount {
    let Some(pass) = sweep_pass(pg).await else {
        return SweepCount::default();
    };
    let count = pass.count();
    for task in &pass.offer {
        // Re-offering is the whole point: every internal gate (agent
        // refusal, the chain order, the generation walk) re-decides from
        // live state.
        maybe_dispatch_ticket(pg, deps, task, None).await;
    }
    let SweepCount {
        offered,
        parked,
        waiting,
    } = count;
    if offered > 0 || parked > 0 || waiting > 0 {
        let parked_note = if parked > 0 {
            format!(" ({parked} parked outside pickup columns)")
        } else {
            String::new()
        };
        let waiting_note = if waiting > 0 {
            format!(" ({waiting} waiting on an earlier workchain step)")
        } else {
            String::new()
        };
        tracing::info!(
            "{LOG} sweep re-offered {offered} ticket(s) to their agents{parked_note}{waiting_note}"
        );
    }
    count
}

/// The sweep's cadence. Sixty seconds, matching the update-reconcile hand:
/// a dead session costs a minute of silence, not an evening. The settle
/// window (not the cadence) is what keeps a just-errored session from
/// burning generations in a loop.
const REDISPATCH_EVERY_MS: u64 = 60_000;

pub fn redispatch_job_spec(
    pg: sqlx::PgPool,
    run: std::sync::Arc<RunDeps>,
) -> talaria_scheduler::JobSpec {
    talaria_scheduler::JobSpec {
        name: talaria_scheduler::JobName::WorkRedispatch,
        every_ms: REDISPATCH_EVERY_MS,
        first_run_delay_ms: Some(90_000),
        max_run_ms: Some(30_000),
        // NOT `per_instance`: the input is the tasks/runs tables, which every
        // instance reaches — the fleet does one sweep per interval, and the
        // generation walk is what makes a racing second sweeper a no-op.
        per_instance: false,
        run: Arc::new(move || {
            let pg = pg.clone();
            let run = run.clone();
            Box::pin(async move {
                redispatch_agent_work(&pg, &run).await;
                Ok(None)
            })
        }),
    }
}

pub fn register_redispatch_job(pg: sqlx::PgPool, run: std::sync::Arc<RunDeps>) {
    talaria_scheduler::register_job(redispatch_job_spec(pg, run));
}

// ── Tests ────────────────────────────────────────────────────────────────────
// The walk is the part worth pinning without a database: stand down on a live
// run, skip a finished one, claim the first empty generation, treat a
// duplicate key as the race resolving, and say the twenty-five sentence. The
// full drive path is the runs engine's own integration suite.

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::BoxFuture;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use talaria_runs_define::{DecisionAnswer, RunDecision, RunRow, RunState};
    use talaria_runs_run::{LeaseClaim, LeaseRenewal, PauseOutcome, RunLease};
    use talaria_runs_store::{
        AnswerOutcome, CancelOutcome, ClaimOutcome, NewRun, RunStore, WriteFailure, WriteOutcome,
    };
    use talaria_runs_work_session::WORK_SESSION_KIND;

    /// A store that answers `get` from a map and records inserts. Two modes
    /// beyond the default: `duplicate_first` refuses the FIRST insert with a
    /// unique violation (the race the walk must read as "somebody claimed
    /// it"), and `blind` fails every read (a store we cannot ask is a
    /// dispatch we do not make). Every method the walk never touches is
    /// unreachable on purpose — louder than a wrong answer if that ever
    /// changes.
    struct WalkStore {
        rows: Mutex<HashMap<String, RunState>>,
        inserted: Mutex<Vec<String>>,
        duplicate_first: bool,
        blind: bool,
    }

    impl WalkStore {
        fn with(rows: HashMap<String, RunState>) -> Arc<Self> {
            Arc::new(Self {
                rows: Mutex::new(rows),
                inserted: Mutex::new(Vec::new()),
                duplicate_first: false,
                blind: false,
            })
        }
        fn empty() -> Arc<Self> {
            Self::with(HashMap::new())
        }
        fn racing_first_insert() -> Arc<Self> {
            Arc::new(Self {
                duplicate_first: true,
                ..Self::bare()
            })
        }
        fn blind() -> Arc<Self> {
            Arc::new(Self {
                blind: true,
                ..Self::bare()
            })
        }
        fn bare() -> Self {
            Self {
                rows: Mutex::new(HashMap::new()),
                inserted: Mutex::new(Vec::new()),
                duplicate_first: false,
                blind: false,
            }
        }
    }

    #[derive(Debug)]
    struct UniqueViolation;

    impl std::fmt::Display for UniqueViolation {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("duplicate key value violates unique constraint")
        }
    }
    impl std::error::Error for UniqueViolation {}
    impl sqlx::error::DatabaseError for UniqueViolation {
        fn message(&self) -> &str {
            "duplicate key value violates unique constraint"
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some("23505".into())
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::UniqueViolation
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
    }

    impl RunStore for WalkStore {
        fn insert<'a>(&'a self, row: NewRun) -> BoxFuture<'a, Result<RunRow, sqlx::Error>> {
            Box::pin(async move {
                if self.duplicate_first && self.inserted.lock().unwrap().is_empty() {
                    return Err(sqlx::Error::Database(Box::new(UniqueViolation)));
                }
                self.inserted.lock().unwrap().push(row.id.clone());
                let state = RunState::Queued;
                self.rows.lock().unwrap().insert(row.id.clone(), state);
                Ok(row_of(row.id, state))
            })
        }
        fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<RunRow>, sqlx::Error>> {
            Box::pin(async move {
                if self.blind {
                    return Err(sqlx::Error::PoolClosed);
                }
                Ok(self
                    .rows
                    .lock()
                    .unwrap()
                    .get(id)
                    .map(|state| row_of(id.to_string(), *state)))
            })
        }
        fn claim<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
            _lease_ms: i64,
        ) -> BoxFuture<'a, Result<ClaimOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn heartbeat<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
            _lease_ms: i64,
        ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn checkpoint<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
            _checkpoint: Value,
            _phase: String,
            _clear_decision: bool,
        ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn phase<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
            _phase: String,
        ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn complete<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
            _result: Value,
        ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn fail<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
            _error: String,
        ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn park<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
            _decision: RunDecision,
            _approval_key: String,
            _phase: String,
        ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn defer<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
            _until_ms: i64,
            _reason: String,
        ) -> BoxFuture<'a, Result<WriteOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn release<'a>(
            &'a self,
            _id: &'a str,
            _token: &'a str,
        ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
            unreachable_driven()
        }
        fn answer<'a>(
            &'a self,
            _id: &'a str,
            _answer: DecisionAnswer,
        ) -> BoxFuture<'a, Result<AnswerOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn cancel<'a>(
            &'a self,
            _id: &'a str,
            _reason: Option<String>,
        ) -> BoxFuture<'a, Result<CancelOutcome, sqlx::Error>> {
            unreachable_driven()
        }
        fn due<'a>(&'a self, _limit: i64) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
            unreachable_driven()
        }
        fn active_for<'a>(
            &'a self,
            _user_id: &'a str,
            _limit: Option<i64>,
        ) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
            unreachable_driven()
        }
    }

    // The walk touches get and insert only: the detached drive stops at
    // NoDefinition (the fake's definition_for answers None) after one get,
    // so every driver-side store method is a panic that names itself.
    fn unreachable_driven<'a, T>() -> BoxFuture<'a, T> {
        Box::pin(async { unreachable!("the dispatch walk drives nothing") })
    }

    /// The lease edge is unreachable for the same reason — no definition, no
    /// claim; a store-only fake keeps the struct literal honest without a
    /// Redis server.
    struct NoLease;
    impl RunLease for NoLease {
        fn acquire<'a>(&'a self, _: &'a str, _: i64) -> BoxFuture<'a, LeaseClaim> {
            Box::pin(async { LeaseClaim::Busy })
        }
        fn renew<'a>(&'a self, _: &'a str, _: &'a str, _: i64) -> BoxFuture<'a, LeaseRenewal> {
            Box::pin(async { LeaseRenewal::Ok })
        }
        fn release<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, ()> {
            Box::pin(async {})
        }
    }

    fn row_of(id: String, state: RunState) -> RunRow {
        RunRow {
            id,
            kind: WORK_SESSION_KIND.into(),
            owner_user_id: None,
            subject_type: Some("task".into()),
            subject_id: Some("t-1".into()),
            state,
            phase: "queued".into(),
            checkpoint: serde_json::Value::Null,
            input: serde_json::Value::Null,
            result: serde_json::Value::Null,
            error: None,
            attempt: 0,
            crashes: 0,
            lease_owner: None,
            lease_expires_at: None,
            approval_key: None,
            decision: None,
            created_at: String::new(),
            updated_at: String::new(),
            started_at: None,
            finished_at: None,
        }
    }

    fn deps(store: Arc<WalkStore>) -> RunDeps {
        RunDeps {
            store,
            lease: Arc::new(NoLease),
            publish: Arc::new(|_, _| {}),
            pause: Arc::new(|_| {
                Box::pin(async {
                    PauseOutcome::Refused {
                        reason: WriteFailure::Missing,
                        state: None,
                    }
                })
            }),
            definition_for: Arc::new(|_| None),
            now: Arc::new(|| 0),
            new_id: Arc::new(|| "new".into()),
        }
    }

    fn ticket() -> DispatchTicket {
        DispatchTicket {
            id: "t-1".into(),
            board_id: "b-1".into(),
            status: "inbox".into(),
            assignees: vec![],
            archived_at: None,
        }
    }

    fn session_id(task: &str, agent: &str, generation: u32) -> String {
        session_run_id(task, agent, generation)
    }

    #[tokio::test]
    async fn claims_generation_zero_when_nothing_exists() {
        let store = WalkStore::empty();
        dispatch_ticket_work(&deps(store.clone()), &ticket(), "claude-opus-4-5", None).await;
        let inserted = store.inserted.lock().unwrap().clone();
        assert_eq!(inserted, vec![session_id("t-1", "claude-opus-4-5", 0)]);
    }

    #[tokio::test]
    async fn stands_down_on_a_live_session_and_skips_a_finished_one() {
        // Generation 0 finished, generation 1 is LIVE: dispatch must stand
        // down at 1 and never look at 2 — that live run IS the session.
        let store = WalkStore::with(HashMap::from([
            (session_id("t-1", "claude-opus-4-5", 0), RunState::Done),
            (session_id("t-1", "claude-opus-4-5", 1), RunState::Running),
        ]));
        dispatch_ticket_work(&deps(store.clone()), &ticket(), "claude-opus-4-5", None).await;
        assert!(store.inserted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_parked_run_is_live_for_this_purpose() {
        // `awaiting` is not terminal: a session parked on a person's decision
        // is still the session, and a second one must not start beside it.
        let store = WalkStore::with(HashMap::from([(
            session_id("t-1", "claude-opus-4-5", 0),
            RunState::Awaiting,
        )]));
        dispatch_ticket_work(&deps(store.clone()), &ticket(), "claude-opus-4-5", None).await;
        assert!(store.inserted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn walks_past_finished_generations_to_the_first_hole() {
        let store = WalkStore::with(HashMap::from([
            (session_id("t-1", "claude-opus-4-5", 0), RunState::Done),
            (session_id("t-1", "claude-opus-4-5", 1), RunState::Error),
            (session_id("t-1", "claude-opus-4-5", 2), RunState::Cancelled),
        ]));
        dispatch_ticket_work(&deps(store.clone()), &ticket(), "claude-opus-4-5", None).await;
        let inserted = store.inserted.lock().unwrap().clone();
        assert_eq!(inserted, vec![session_id("t-1", "claude-opus-4-5", 3)]);
    }

    #[tokio::test]
    async fn a_duplicate_key_is_the_race_resolving_not_a_failure() {
        let store = WalkStore::racing_first_insert();
        dispatch_ticket_work(&deps(store.clone()), &ticket(), "claude-opus-4-5", None).await;
        // One refusal, no retry: what the other dispatcher inserted is a live
        // session for this exact ticket and agent.
        assert!(store.inserted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_store_means_no_dispatch() {
        let store = WalkStore::blind();
        // Must return without panicking — the driven-method unreachables are
        // the assertion that a blind store means no insert.
        dispatch_ticket_work(&deps(store), &ticket(), "claude-opus-4-5", None).await;
    }

    // C13: the sweep offers a ticket only to an AGENT. A people-only ticket
    // has nobody `maybe_dispatch_ticket` can loop over, so it is neither
    // offered nor counted; a mixed one is offered with its whole list.
    #[test]
    fn a_people_only_ticket_is_not_the_sweeps_to_offer() {
        let people = serde_json::json!(["user:7d3c0d7e-2a8e-4c35-9a61-3b1f5e0c9a10"]);
        assert_eq!(sweep_assignees(Some(people)), None);
        let mixed = serde_json::json!(["user:7d3c0d7e-2a8e-4c35-9a61-3b1f5e0c9a10", "ada-ops"]);
        assert_eq!(
            sweep_assignees(Some(mixed)),
            Some(vec![
                "user:7d3c0d7e-2a8e-4c35-9a61-3b1f5e0c9a10".to_string(),
                "ada-ops".to_string()
            ])
        );
        assert_eq!(
            sweep_assignees(Some(serde_json::json!(["ada-ops"]))),
            Some(vec!["ada-ops".to_string()])
        );
        assert_eq!(sweep_assignees(Some(serde_json::json!([]))), None);
        assert_eq!(sweep_assignees(Some(Value::Null)), None);
        assert_eq!(sweep_assignees(None), None);
        // Not an array of strings: json_strings reads nothing, so nothing is
        // offered (the same law the dispatch gate reads it with).
        assert_eq!(sweep_assignees(Some(serde_json::json!({"a": 1}))), None);
    }
}

// ── Live DB: the sweep's own query against the real schema ──────────────────
// What a unit test cannot see is the pass reading real rows: the JSONB
// assignees column, the default status list a board with no custom columns
// gets, and the pickup-column filter. #[ignore]d; CI's live job runs it on its
// migrated scratch database:
//
//   DATABASE_URL=postgres://… cargo test -p talaria-work-dispatch --lib -- --ignored live_db
//
// The pass reads every ticket in the database, so the assertions are on this
// test's own rows only; everything it writes hangs off one user row it
// deletes at the end (users → boards → tasks cascade).
#[cfg(test)]
mod live_db {
    use super::*;

    async fn pg() -> PgPool {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        PgPool::connect(&url).await.expect("connect")
    }

    async fn ticket(
        pg: &PgPool,
        board: &str,
        status: &str,
        assignees: serde_json::Value,
    ) -> String {
        sqlx::query_scalar(
            "insert into tasks (board_id, title, status, assignees) \
             values ($1::uuid, 'sweep-live', $2, $3) returning id::text",
        )
        .bind(board)
        .bind(status)
        .bind(assignees)
        .fetch_one(pg)
        .await
        .expect("insert ticket")
    }

    #[tokio::test]
    #[ignore = "needs a migrated Postgres (DATABASE_URL)"]
    async fn the_pass_offers_agent_tickets_and_skips_people_only_ones() {
        let pg = pg().await;
        let tag = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let email = format!("sweep-live-{tag}@link-test.invalid");
        let user: String = sqlx::query_scalar(
            "insert into users (sub, email, name, role) values ($1, $1, 'Sweep Live', 'member') \
             returning id::text",
        )
        .bind(&email)
        .fetch_one(&pg)
        .await
        .expect("insert user");
        let board: String = sqlx::query_scalar(
            "insert into boards (name, owner_id) values ('sweep-live', $1::uuid) returning id::text",
        )
        .bind(&user)
        .fetch_one(&pg)
        .await
        .expect("insert board");
        // `assigned` is a pickup column of the default list; `inbox` is not.
        let person = format!("user:{user}");
        let people_only = ticket(&pg, &board, "assigned", serde_json::json!([person])).await;
        let agent = ticket(
            &pg,
            &board,
            "assigned",
            serde_json::json!(["sweep-live-agent"]),
        )
        .await;
        let mixed = ticket(
            &pg,
            &board,
            "in_progress",
            serde_json::json!([person, "sweep-live-agent"]),
        )
        .await;
        let parked = ticket(
            &pg,
            &board,
            "inbox",
            serde_json::json!(["sweep-live-agent"]),
        )
        .await;
        let people_parked = ticket(&pg, &board, "inbox", serde_json::json!([person])).await;

        let pass = sweep_pass(&pg).await.expect("the pass runs");
        let mine: Vec<String> = vec![
            people_only.clone(),
            agent.clone(),
            mixed.clone(),
            parked.clone(),
            people_parked.clone(),
        ];
        let ours = |ids: Vec<String>| -> Vec<String> {
            let mut ids: Vec<String> = ids.into_iter().filter(|i| mine.contains(i)).collect();
            ids.sort();
            ids
        };
        let offered = ours(pass.offer.iter().map(|t| t.id.clone()).collect());
        let mut expect_offered = vec![agent.clone(), mixed.clone()];
        expect_offered.sort();
        assert_eq!(
            offered, expect_offered,
            "only the agent-assigned pickup tickets are offered"
        );
        assert_eq!(ours(pass.parked.clone()), vec![parked.clone()]);
        assert!(ours(pass.waiting.clone()).is_empty());
        // The mixed ticket rides with its whole list; dispatch filters again.
        let mixed_row = pass.offer.iter().find(|t| t.id == mixed).unwrap();
        assert_eq!(
            mixed_row.assignees,
            vec![person.clone(), "sweep-live-agent".to_string()]
        );

        sqlx::query("delete from users where id = $1::uuid")
            .bind(&user)
            .execute(&pg)
            .await
            .expect("cleanup");
    }
}
