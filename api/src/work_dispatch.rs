// Work dispatch — THE PUSH SIDE. When a ticket enters an agent-start column
// with agent assignees, Talaria pushes the work into the agent's own persona
// gateway as a WORK SESSION: turn after turn, the agent drives its
// tools/harness like a developer at a desk (run, read, steer, test, repeat),
// until the ticket reaches review/blocked/done, the agent stops progressing,
// or the turn cap lands. One turn is never the budget for real work. Matched
// task WORKFLOWS ride along; the plugin heartbeat remains the pull-side
// safety net.
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

use crate::runs::define::is_terminal;
use crate::runs::defs::work_session::{session_run_id, work_session_run};
use crate::runs::run::{EnqueueOptions, RunDeps, enqueue};
use crate::{realtime, statuses, tasks};
use sqlx::PgPool;

const LOG: &str = "[work-dispatch]";

/// How many sessions one ticket+agent pair may ever have.
///
/// Not a rate limit — it is the bound on the generation walk below, and it is
/// a SIGNAL as much as a guard: a ticket that has been handed to the same
/// agent twenty-five times is a ticket bouncing between a column and an agent
/// that cannot finish it, and the honest response is to say so loudly and
/// stop rather than to quietly start a twenty-sixth.
pub const MAX_SESSIONS_PER_TICKET_AGENT: u32 = 25;

/// A primary-key collision, and the only error `enqueue` can raise that means
/// something rather than being broken: somebody else claimed this exact
/// session in the time between our read and our insert. SQLSTATE 23505 is
/// `unique_violation`. Same spelling as password_accounts' login collision.
fn is_duplicate_key(e: &sqlx::Error) -> bool {
    e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23505")
}

/// The task fields dispatch reads — exactly the five the push side depends
/// on. The full row is tasks.rs's Task, which converts into this.
pub struct DispatchTicket {
    pub id: String,
    pub board_id: String,
    pub status: String,
    pub assignees: Vec<String>,
    pub archived_at: Option<String>,
}

/// The dispatch assembly: the FULL real one, `real_run_deps`. A run that
/// parks a question actually parks, because the driver that will park it is
/// this process.
pub fn dispatch_deps(
    pg: PgPool,
    redis: redis::aio::ConnectionManager,
    rt: realtime::RealtimeDeps,
) -> RunDeps {
    crate::runs::real_run_deps(pg, redis, rt)
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
) {
    let def = work_session_run();
    for generation in 0..MAX_SESSIONS_PER_TICKET_AGENT {
        let id = session_run_id(&task.id, agent_model, generation);
        let existing = match deps.store.get(&id).await {
            Ok(row) => row,
            Err(e) => {
                // The claim is the only thing standing between a retried
                // request and two agents on one ticket, so a store we cannot
                // read is a dispatch we do not make. The heartbeat is the
                // pull-side safety net for exactly this.
                tracing::error!(
                    "{LOG} {}: could not check for a live session with {agent_model}, not dispatching: {e}",
                    task.id
                );
                return;
            }
        };
        match existing {
            // A live (or parked) run at this generation IS the session.
            Some(row) if !is_terminal(row.state) => return,
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
            Ok(_) => return,
            // Somebody else claimed this exact session between our read and
            // our insert — and what they inserted is a live session for this
            // ticket and agent, so standing down is correct.
            Err(e) if is_duplicate_key(&e) => return,
            Err(e) => {
                tracing::error!(
                    "{LOG} {}: could not start a work session with {agent_model}: {e}",
                    task.id
                );
                return;
            }
        }
    }
    tracing::error!(
        "{LOG} {} has had {MAX_SESSIONS_PER_TICKET_AGENT} work sessions with {agent_model} and \
         every one of them finished — not starting another. This ticket keeps coming back to an \
         agent that cannot close it; a person should look at it.",
        task.id
    );
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
/// IT IS STILL ASKED AGAIN INSIDE THE SESSION, on every turn. This gate is
/// about whether to START; a run may sit in the queue, park, or be reclaimed
/// after a deploy, and the first thing every step does is put the same
/// question to `agent_ticket_refusal` again.
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
        let subject = crate::agent_auth::AgentSubject::Model(agent.clone());
        match tasks::agent_ticket_refusal(pg, &target, &subject, tasks::AgentIntent::Write).await {
            Ok(Some(_refusal)) => continue,
            Ok(None) => {
                dispatch_ticket_work(deps, task, &agent, None).await;
            }
            Err(e) => {
                tracing::error!("{LOG} {}: dispatch to {agent} threw: {e}", task.id);
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
// The walk is the part worth pinning without a database: stand down on a live
// run, skip a finished one, claim the first empty generation, treat a
// duplicate key as the race resolving, and say the twenty-five sentence. The
// full drive path is the runs engine's own integration suite.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::define::{DecisionAnswer, RunDecision, RunRow, RunState};
    use crate::runs::defs::work_session::WORK_SESSION_KIND;
    use crate::runs::run::{LeaseClaim, LeaseRenewal, PauseOutcome, RunLease};
    use crate::runs::store::{
        AnswerOutcome, CancelOutcome, ClaimOutcome, NewRun, RunStore, WriteFailure, WriteOutcome,
    };
    use futures_util::future::BoxFuture;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;

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
}
