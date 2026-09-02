// PAUSE AS APPROVAL, end to end: a run parks on a question, the people its
// definition names are told, somebody who is not one of them is refused, the
// person who is answers, and the run goes back to work carrying the answer.
//
// The store and the lease are in-memory and REIMPLEMENT the compare-and-set
// predicates rather than accepting whatever they are told — "a paused run
// cannot be driven" and "a decision cannot come from a stranger" are properties
// of those predicates and of the authority check, and a fake that wrote
// whatever it was handed would turn every assertion below into a restatement
// of the fake. The live suite (runs_store.rs) proves the same predicates
// against the real table.
//
// The announce-and-notify leg — `sweep_unannounced`, in the approvals module —
// is tested with that module. What it consumes is asserted here: the
// `PendingApproval` the census builds from the parked row, which is the exact
// input the sweep takes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::{FutureExt, future::BoxFuture};
use serde_json::{Value, json};
use talaria_api::agent_auth::{epoch_ms_to_iso, iso_to_epoch_ms};
use talaria_api::approvals::{ApprovalKind, Disclosure, run_decision_approval};
use talaria_api::runs::decide::{
    AnnounceFn, AudienceForFn, DecideArgs, DecideDeps, DecideRefusal, DecideResult, PauseDeps,
    PauseResult, run_approval_key,
};
use talaria_api::runs::define::{
    Authority, DecisionAnswer, DecisionOption, DecisionRequest, RunDecision, RunDefinition, RunRow,
    RunState, StepResult,
};
use talaria_api::runs::run::{
    DefinitionForFn, DriveStop, LeaseClaim, LeaseRenewal, PauseArgs, RunDeps, RunEvent, RunLease,
    drive,
};
use talaria_api::runs::store::{
    AnswerOutcome, CancelOutcome, ClaimOutcome, NewRun, RunStore, WriteFailure,
};

const NOW: i64 = 1_700_000_000_000;

// ── The fake world ───────────────────────────────────────────────────────────

/// The same compare-and-set predicates the store spells in SQL. Narrower than
/// runs_run.rs's copy — only the writes these two halves reach — but not
/// looser: every write still requires `lease_owner = token and
/// state = 'running'`, which is the only reason a park cannot be raced and a
/// paused run cannot be driven.
struct MemoryStore {
    rows: Mutex<HashMap<String, RunRow>>,
}

impl MemoryStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            rows: Mutex::new(HashMap::new()),
        })
    }

    fn iso(at: i64) -> String {
        epoch_ms_to_iso(at)
    }

    fn row(self: &Arc<Self>, id: &str) -> RunRow {
        self.rows
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .expect("seeded row")
    }

    /// The refusal every guarded write returns for this row.
    fn refusal(row: &RunRow) -> WriteFailure {
        if row.state == RunState::Cancelled {
            WriteFailure::Cancelled
        } else if row.lease_owner.is_none() {
            WriteFailure::State { state: row.state }
        } else {
            WriteFailure::LeaseLost { state: row.state }
        }
    }

    /// The compare-and-set under every guarded write.
    fn cas(
        &self,
        id: &str,
        token: &str,
        mutate: impl FnOnce(&mut RunRow),
    ) -> Result<(), WriteFailure> {
        let mut rows = self.rows.lock().unwrap();
        let Some(row) = rows.get_mut(id) else {
            return Err(WriteFailure::Missing);
        };
        if row.state == RunState::Cancelled {
            return Err(WriteFailure::Cancelled);
        }
        if row.lease_owner.as_deref() != Some(token) {
            return Err(Self::refusal(row));
        }
        if row.state != RunState::Running {
            return Err(WriteFailure::State { state: row.state });
        }
        mutate(row);
        row.updated_at = Self::iso(NOW);
        Ok(())
    }

    fn plant(self: &Arc<Self>, state: RunState, lease: Option<&str>, started: bool) -> RunRow {
        let row = RunRow {
            id: "run-1".into(),
            kind: "test-decision-run".into(),
            owner_user_id: Some("u-owner".into()),
            subject_type: Some("task".into()),
            subject_id: Some("board-1".into()),
            state,
            phase: "picking an assignee".into(),
            checkpoint: Value::Null,
            input: json!({ "taskId": "task-1" }),
            result: Value::Null,
            error: None,
            attempt: 0,
            lease_owner: lease.map(str::to_string),
            lease_expires_at: lease.map(|_| Self::iso(NOW + 30_000)),
            approval_key: None,
            decision: None,
            created_at: Self::iso(NOW),
            updated_at: Self::iso(NOW),
            started_at: started.then(|| Self::iso(NOW)),
            finished_at: None,
        };
        self.rows
            .lock()
            .unwrap()
            .insert(row.id.clone(), row.clone());
        row
    }

    /// A run already claimed and stepping, which is the only state a pause can
    /// legally happen from.
    fn running(self: &Arc<Self>) -> RunRow {
        self.plant(RunState::Running, Some("tok-1"), true)
    }

    fn running_held_by(self: &Arc<Self>, owner: &str) -> RunRow {
        self.plant(RunState::Running, Some(owner), true)
    }

    /// A fresh run nobody has claimed — so the DRIVER takes the lease and runs
    /// the step, and the pause is the one the step actually asked for.
    fn queued_fresh(self: &Arc<Self>) -> RunRow {
        self.plant(RunState::Queued, None, false)
    }
}

impl RunStore for MemoryStore {
    fn insert<'a>(&'a self, row: NewRun) -> BoxFuture<'a, Result<RunRow, sqlx::Error>> {
        Box::pin(async move {
            let full = RunRow {
                state: RunState::Queued,
                attempt: 0,
                error: None,
                result: Value::Null,
                checkpoint: Value::Null,
                lease_owner: None,
                lease_expires_at: None,
                approval_key: None,
                decision: None,
                started_at: None,
                finished_at: None,
                created_at: Self::iso(NOW),
                updated_at: Self::iso(NOW),
                id: row.id,
                kind: row.kind,
                owner_user_id: row.owner_user_id,
                subject_type: row.subject_type,
                subject_id: row.subject_id,
                input: row.input,
                phase: row.phase,
            };
            self.rows
                .lock()
                .unwrap()
                .insert(full.id.clone(), full.clone());
            Ok(full)
        })
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<RunRow>, sqlx::Error>> {
        Box::pin(async move { Ok(self.rows.lock().unwrap().get(id).cloned()) })
    }

    fn claim<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        lease_ms: i64,
    ) -> BoxFuture<'a, Result<ClaimOutcome, sqlx::Error>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            let Some(mut row) = rows.get(id).cloned() else {
                return Ok(ClaimOutcome::Missing);
            };
            if !matches!(row.state, RunState::Queued | RunState::Running) {
                return Ok(ClaimOutcome::NotRunnable { state: row.state });
            }
            let live = row
                .lease_expires_at
                .as_deref()
                .and_then(iso_to_epoch_ms)
                .is_some_and(|e| e > NOW);
            if live {
                return Ok(ClaimOutcome::Taken {
                    state: row.state,
                    until: row.lease_expires_at.clone(),
                });
            }
            let reclaimed = row.state == RunState::Running;
            if reclaimed {
                row.attempt += 1;
            }
            row.state = RunState::Running;
            row.lease_owner = Some(token.to_string());
            row.lease_expires_at = Some(Self::iso(NOW + lease_ms));
            if row.started_at.is_none() {
                row.started_at = Some(Self::iso(NOW));
            }
            rows.insert(id.to_string(), row.clone());
            Ok(ClaimOutcome::Claimed {
                run: Box::new(row),
                reclaimed,
            })
        })
    }

    fn heartbeat<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        lease_ms: i64,
    ) -> BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            Ok(self.cas(id, token, |row| {
                row.lease_expires_at = Some(Self::iso(NOW + lease_ms))
            }))
        })
    }

    fn checkpoint<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        checkpoint: Value,
        phase: String,
        clear_decision: bool,
    ) -> BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            Ok(self.cas(id, token, |row| {
                row.checkpoint = checkpoint;
                row.phase = phase;
                if clear_decision {
                    row.decision = None;
                    row.approval_key = None;
                }
            }))
        })
    }

    fn phase<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        phase: String,
    ) -> BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move { Ok(self.cas(id, token, |row| row.phase = phase)) })
    }

    fn complete<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        result: Value,
    ) -> BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            Ok(self.cas(id, token, |row| {
                row.state = RunState::Done;
                row.result = result;
                row.decision = None;
                row.approval_key = None;
                row.lease_owner = None;
                row.lease_expires_at = None;
                row.finished_at = Some(Self::iso(NOW));
            }))
        })
    }

    fn fail<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        error: String,
    ) -> BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            Ok(self.cas(id, token, |row| {
                row.state = RunState::Error;
                row.error = Some(error);
                row.lease_owner = None;
                row.lease_expires_at = None;
                row.finished_at = Some(Self::iso(NOW));
            }))
        })
    }

    fn park<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        decision: RunDecision,
        approval_key: String,
        phase: String,
    ) -> BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            Ok(self.cas(id, token, |row| {
                row.state = RunState::Awaiting;
                row.decision = Some(decision);
                row.approval_key = Some(approval_key);
                row.phase = phase;
                row.lease_owner = None; // the row is nobody's while it waits
                row.lease_expires_at = None;
            }))
        })
    }

    fn defer<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        until_ms: i64,
        reason: String,
    ) -> BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            Ok(self.cas(id, token, |row| {
                row.state = RunState::Queued;
                row.phase = reason;
                // The lease stamp IS the wait — the owner stays set so the row
                // says who deferred it.
                row.lease_expires_at = Some(Self::iso(until_ms));
            }))
        })
    }

    fn release<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
    ) -> BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            if let Some(row) = rows.get_mut(id)
                && row.lease_owner.as_deref() == Some(token)
                && row.state == RunState::Running
            {
                row.lease_owner = None;
                row.lease_expires_at = None;
            }
            Ok(())
        })
    }

    fn answer<'a>(
        &'a self,
        id: &'a str,
        answer: DecisionAnswer,
    ) -> BoxFuture<'a, Result<AnswerOutcome, sqlx::Error>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            let Some(mut row) = rows.get(id).cloned() else {
                return Ok(AnswerOutcome::Missing);
            };
            if row.state != RunState::Awaiting {
                return Ok(AnswerOutcome::NotAwaiting {
                    state: Some(row.state),
                });
            }
            let Some(d) = row.decision.as_ref() else {
                return Ok(AnswerOutcome::NotAwaiting {
                    state: Some(row.state),
                });
            };
            if d.request.key != answer.key {
                return Ok(AnswerOutcome::StaleKey { state: row.state });
            }
            // The SQL keeps the approval key; the census re-derives nothing,
            // and `run_decision_approval` already returns None off the state.
            row.decision = Some(RunDecision {
                request: d.request.clone(),
                answer: Some(answer),
            });
            row.state = RunState::Queued;
            row.lease_owner = None;
            row.lease_expires_at = None;
            row.updated_at = Self::iso(NOW);
            rows.insert(id.to_string(), row.clone());
            Ok(AnswerOutcome::Answered(Box::new(row)))
        })
    }

    fn cancel<'a>(
        &'a self,
        id: &'a str,
        reason: Option<String>,
    ) -> BoxFuture<'a, Result<CancelOutcome, sqlx::Error>> {
        Box::pin(async move {
            let mut rows = self.rows.lock().unwrap();
            let Some(row) = rows.get_mut(id) else {
                return Ok(CancelOutcome::Missing);
            };
            if !matches!(
                row.state,
                RunState::Queued | RunState::Running | RunState::Awaiting
            ) {
                return Ok(CancelOutcome::Terminal { state: row.state });
            }
            let was = row.state;
            row.state = RunState::Cancelled;
            row.error = reason;
            row.lease_owner = None;
            row.lease_expires_at = None;
            row.finished_at = Some(Self::iso(NOW));
            Ok(CancelOutcome::Cancelled { state: was })
        })
    }

    fn due<'a>(&'a self, _limit: i64) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
        Box::pin(async move { Ok(vec![]) })
    }

    fn active_for<'a>(
        &'a self,
        _user_id: &'a str,
        _limit: Option<i64>,
    ) -> BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
        Box::pin(async move { Ok(vec![]) })
    }
}

struct MemoryLease {
    held: Mutex<HashMap<String, String>>,
    n: std::sync::atomic::AtomicUsize,
}

impl MemoryLease {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            held: Mutex::new(HashMap::new()),
            n: std::sync::atomic::AtomicUsize::new(0),
        })
    }
}

impl RunLease for MemoryLease {
    fn acquire<'a>(
        &'a self,
        run_id: &'a str,
        _step_ms: i64,
    ) -> futures_util::future::BoxFuture<'a, LeaseClaim> {
        Box::pin(async move {
            let mut held = self.held.lock().unwrap();
            if held.contains_key(run_id) {
                return LeaseClaim::Busy;
            }
            let n = self.n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let token = format!("lease-{n}");
            held.insert(run_id.to_string(), token.clone());
            LeaseClaim::Claimed { token }
        })
    }

    fn renew<'a>(
        &'a self,
        run_id: &'a str,
        token: &'a str,
        _step_ms: i64,
    ) -> futures_util::future::BoxFuture<'a, LeaseRenewal> {
        Box::pin(async move {
            match self.held.lock().unwrap().get(run_id) {
                Some(t) if t == token => LeaseRenewal::Ok,
                _ => LeaseRenewal::Lost,
            }
        })
    }

    fn release<'a>(
        &'a self,
        run_id: &'a str,
        token: &'a str,
    ) -> futures_util::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            let mut held = self.held.lock().unwrap();
            if held.get(run_id).is_some_and(|t| t == token) {
                held.remove(run_id);
            }
        })
    }
}

// ── The run under test ───────────────────────────────────────────────────────

fn ask() -> DecisionRequest {
    DecisionRequest {
        key: "assignee".into(),
        question: "Two people are assigned — who should take this ticket?".into(),
        detail: Some("Both are editors on the board and neither has started.".into()),
        options: vec![
            DecisionOption {
                id: "ana".into(),
                label: "Ana".into(),
                detail: Some("Has the most context".into()),
            },
            DecisionOption {
                id: "ben".into(),
                label: "Ben".into(),
                detail: None,
            },
        ],
        href: Some("/boards/board-1/task-1".into()),
    }
}

/// Entered with a decision the person gave, so an assertion can prove the
/// answer reached the step rather than merely reaching the row.
type StepsSaw = Arc<Mutex<Vec<Option<DecisionAnswer>>>>;

/// THE AUTHORITY BOUNDARY, from the run's side: it names an authority and knows
/// nothing else about access. This one pauses to the ticket's BOARD.
fn handover(steps_saw: StepsSaw) -> Arc<RunDefinition> {
    Arc::new(RunDefinition {
        kind: "test-decision-run".into(),
        label: "Ticket handover".into(),
        step: Arc::new(move |ctx| {
            let saw = steps_saw.clone();
            Box::pin(async move {
                saw.lock().unwrap().push(ctx.decision.clone());
                if let Some(a) = ctx.decision {
                    Ok(StepResult::Done {
                        result: json!({ "picked": a.option_id }),
                    })
                } else {
                    Ok(StepResult::Decide { question: ask() })
                }
            })
        }),
        audience: Arc::new(|run| Authority::Board {
            board_id: run.subject_id.clone().unwrap_or_else(|| "unknown".into()),
        }),
        max_step_ms: 30_000,
        max_attempts: 3,
    })
}

struct World {
    store: Arc<MemoryStore>,
    lease: Arc<MemoryLease>,
    events: Arc<Mutex<Vec<RunEvent>>>,
    announced: Arc<Mutex<Vec<String>>>,
    asked_for: Arc<Mutex<Vec<Authority>>>,
    steps_saw: StepsSaw,
    deps: DecideDeps,
}

/// `content` is who the board resolves to; `reached` is what the announcer says
/// it managed to tell, so a test can put the announcement on the floor.
fn world(content: &[&str], reached: usize) -> World {
    let store = MemoryStore::new();
    let lease = MemoryLease::new();
    let events: Arc<Mutex<Vec<RunEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let announced: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let asked_for: Arc<Mutex<Vec<Authority>>> = Arc::new(Mutex::new(Vec::new()));
    let steps_saw: StepsSaw = Arc::new(Mutex::new(Vec::new()));

    let content: Vec<String> = content.iter().map(|s| (*s).to_string()).collect();
    let fact: Vec<String> = if content.is_empty() {
        vec!["u-admin".into()]
    } else {
        vec![]
    };

    let def = handover(steps_saw.clone());
    let defs = HashMap::from([(def.kind.clone(), def)]);
    let definition_for: DefinitionForFn = Arc::new(move |kind| defs.get(kind).cloned());

    let publish = {
        let events = events.clone();
        Arc::new(move |ev: RunEvent, _owner: Option<&str>| {
            events.lock().unwrap().push(ev);
        })
    };

    let audience_for: AudienceForFn = {
        let asked_for = asked_for.clone();
        let content = content.clone();
        let fact = fact.clone();
        Arc::new(move |authority: &Authority| {
            asked_for.lock().unwrap().push(authority.clone());
            let content = content.clone();
            let fact = fact.clone();
            async move { Disclosure { content, fact } }.boxed()
        })
    };

    let announce: AnnounceFn = {
        let announced = announced.clone();
        Arc::new(move |key: &str| {
            announced.lock().unwrap().push(key.to_string());
            let reached = reached;
            async move { reached }.boxed()
        })
    };

    let mark_brief_stale = Arc::new(|_stale: Vec<String>| async {}.boxed());

    // THE CONSTRUCTION ORDER: the pause is built from the edges first, so the
    // `RunDeps` the driver holds carries the real `pause` and the `DecideDeps`
    // the answer half holds wraps that same bag — one world, not two dep
    // graphs that happen to share fakes.
    let pause = talaria_api::runs::decide::pause_fn(PauseDeps {
        store: store.clone(),
        publish: publish.clone(),
        definition_for: definition_for.clone(),
        audience_for: audience_for.clone(),
        announce: announce.clone(),
    });

    let run = RunDeps {
        store: store.clone(),
        lease: lease.clone(),
        publish: publish.clone(),
        pause,
        definition_for: definition_for.clone(),
        now: Arc::new(|| NOW),
        new_id: Arc::new(|| "generated".into()),
    };

    let approval_for = {
        let lookup = definition_for.clone();
        Arc::new(move |row: &RunRow| run_decision_approval(row, &lookup))
    };

    let deps = DecideDeps {
        run,
        approval_for,
        audience_for,
        announce,
        mark_brief_stale,
    };
    World {
        store,
        lease,
        events,
        announced,
        asked_for,
        steps_saw,
        deps,
    }
}

fn pause_args() -> PauseArgs {
    PauseArgs {
        run_id: "run-1".into(),
        token: "tok-1".into(),
        question: ask(),
        phase: None,
    }
}

/// A refusal, unpacked for asserting on.
fn refused(res: &DecideResult) -> (DecideRefusal, Option<RunState>) {
    match res {
        DecideResult::Refused { reason, state } => (*reason, *state),
        DecideResult::Decided { .. } => panic!("expected a refusal, got a decision"),
    }
}

/// A world with a run parked on ASK by the driver that held it, the event log
/// and step log cleared so a test sees only what it causes.
async fn parked(content: &[&str]) -> World {
    let w = world(content, 1);
    let run = w.store.running();
    let res = talaria_api::runs::decide::pause(pause_args(), &w.deps.pause_deps())
        .await
        .unwrap();
    assert!(
        matches!(res, PauseResult::Parked { .. }),
        "the park should have landed"
    );
    w.steps_saw.lock().unwrap().clear();
    w.events.lock().unwrap().clear();
    let _ = run;
    w
}

// ── pause ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn parks_the_run_and_files_the_approval_to_the_audience_the_definition_declares() {
    let w = world(&["u-editor"], 1);
    let run = w.store.running();

    let res = talaria_api::runs::decide::pause(pause_args(), &w.deps.pause_deps())
        .await
        .unwrap();

    let PauseResult::Parked {
        approval_key,
        announced,
        audience,
    } = res
    else {
        panic!("the park should have landed");
    };
    // The key is derived from the run and the question and nothing else, so a
    // re-ask after a reclaim produces the same one and dedupes.
    assert_eq!(approval_key, "run:test-decision-run:run-1:assignee");
    assert_eq!(
        approval_key,
        run_approval_key(&run.kind, &run.id, "assignee")
    );
    assert_eq!(announced, 1);
    assert_eq!(*w.announced.lock().unwrap(), vec![approval_key.clone()]);
    // Resolved from the DEFINITION's authority — the board the run's subject is
    // on, not the run's owner and not the admins.
    assert_eq!(
        *w.asked_for.lock().unwrap(),
        vec![Authority::Board {
            board_id: "board-1".into()
        }]
    );
    assert_eq!(audience.content, vec!["u-editor".to_string()]);

    let row = w.store.row(&run.id);
    assert_eq!(row.state, RunState::Awaiting);
    assert_eq!(row.approval_key.as_deref(), Some(approval_key.as_str()));
    let d = row.decision.as_ref().unwrap();
    assert_eq!(d.request.key, "assignee");
    assert!(d.answer.is_none());
    // The lease is given back: nobody is driving a run that is waiting for a
    // person, and a lease held across a human's lunch break would either
    // expire and look reclaimable or have to be renewed by a process with no
    // work.
    assert!(row.lease_owner.is_none());

    // Persisted, THEN published — and the question rides along so a device can
    // raise it without a round trip.
    let parked = w.events.lock().unwrap().last().cloned().unwrap();
    assert_eq!(parked.state, RunState::Awaiting);
    assert_eq!(
        parked.question.as_ref().map(|q| q.key.as_str()),
        Some("assignee")
    );
}

#[tokio::test]
async fn refuses_to_park_a_run_this_driver_no_longer_holds_and_tells_nobody() {
    let w = world(&["u-editor"], 1);
    let run = w.store.running_held_by("somebody-else");

    let res = talaria_api::runs::decide::pause(pause_args(), &w.deps.pause_deps())
        .await
        .unwrap();

    assert!(matches!(
        res,
        PauseResult::Refused {
            reason: WriteFailure::LeaseLost {
                state: RunState::Running
            }
        }
    ));
    assert!(w.announced.lock().unwrap().is_empty());
    assert_eq!(w.store.row(&run.id).state, RunState::Running);
}

#[tokio::test]
async fn keeps_the_run_parked_when_nobody_could_be_told_and_leaves_the_key_unmarked_for_the_sweep()
{
    // `reached` 0: the announcement went nowhere. The row is still the record —
    // a delivery that did not happen must not destroy the thing it was about.
    let w = world(&[], 0);
    let run = w.store.running();

    let res = talaria_api::runs::decide::pause(pause_args(), &w.deps.pause_deps())
        .await
        .unwrap();

    let PauseResult::Parked { announced, .. } = res else {
        panic!("the park should have landed");
    };
    assert_eq!(announced, 0);
    assert_eq!(w.store.row(&run.id).state, RunState::Awaiting);
}

// ── a paused run cannot self-resume ──────────────────────────────────────────

#[tokio::test]
async fn is_not_drivable_only_a_decision_moves_it_out_of_awaiting() {
    let w = world(&["u-editor"], 1);
    let run = w.store.running();
    talaria_api::runs::decide::pause(pause_args(), &w.deps.pause_deps())
        .await
        .unwrap();
    w.steps_saw.lock().unwrap().clear();

    let result = drive(&run.id, &w.deps.run).await.unwrap();

    assert_eq!(result.stop, DriveStop::NotRunnable);
    assert_eq!(result.steps, 0);
    // The step was never re-entered, so the question was not asked a second
    // time and no side effect of it repeated.
    assert!(w.steps_saw.lock().unwrap().is_empty());
    assert_eq!(w.store.row(&run.id).state, RunState::Awaiting);
    // It never even took the lease.
    assert_eq!(w.lease.held.lock().unwrap().len(), 0);
}

// ── decide ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn refuses_somebody_the_runs_authority_does_not_name() {
    let w = parked(&["u-editor"]).await;

    let res = talaria_api::runs::decide::decide(
        DecideArgs {
            run_id: "run-1".into(),
            option_id: "ana".into(),
            note: None,
            by: "u-stranger".into(),
            start: Some(false),
        },
        &w.deps,
    )
    .await
    .unwrap();

    assert_eq!(
        refused(&res),
        (DecideRefusal::Forbidden, Some(RunState::Awaiting))
    );
    let row = w.store.row("run-1");
    assert_eq!(row.state, RunState::Awaiting);
    assert!(row.decision.as_ref().unwrap().answer.is_none());
    // Nothing about the question was published to anybody on the way out.
    assert!(w.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refuses_an_option_the_step_never_offered() {
    let w = parked(&["u-editor"]).await;

    let res = talaria_api::runs::decide::decide(
        DecideArgs {
            run_id: "run-1".into(),
            option_id: "delete-the-board".into(),
            note: None,
            by: "u-editor".into(),
            start: Some(false),
        },
        &w.deps,
    )
    .await
    .unwrap();

    // The decider is entitled to answer and still cannot hand the step an
    // instruction it wrote no branch for: the answer is DATA, drawn from the
    // options the step declared.
    assert_eq!(
        refused(&res),
        (DecideRefusal::UnknownOption, Some(RunState::Awaiting))
    );
    assert!(
        w.store
            .row("run-1")
            .decision
            .as_ref()
            .unwrap()
            .answer
            .is_none()
    );
}

#[tokio::test]
async fn writes_the_answer_onto_the_run_requeues_it_and_hands_it_to_the_next_step() {
    let w = parked(&["u-editor"]).await;

    let res = talaria_api::runs::decide::decide(
        DecideArgs {
            run_id: "run-1".into(),
            option_id: "ana".into(),
            note: Some("  Ana has the context  ".into()),
            by: "u-editor".into(),
            start: Some(false),
        },
        &w.deps,
    )
    .await
    .unwrap();

    let DecideResult::Decided { .. } = &res else {
        panic!("the decision should have landed, got {res:?}");
    };
    let row = w.store.row("run-1");
    assert_eq!(row.state, RunState::Queued);
    let answer = row.decision.as_ref().unwrap().answer.as_ref().unwrap();
    assert_eq!(answer.key, "assignee");
    assert_eq!(answer.option_id, "ana");
    assert_eq!(answer.note.as_deref(), Some("Ana has the context"));
    assert_eq!(answer.answered_by.as_deref(), Some("u-editor"));
    assert_eq!(answer.answered_at, epoch_ms_to_iso(NOW));
    // Back in the queue with the lease clear: any instance may pick it up.
    assert!(row.lease_owner.is_none());

    // And the answer reaches the STEP, which is the only place it means
    // anything — then is cleared by the write that finishes the run, so a
    // reclaim cannot hand it to a step a second time.
    let driven = drive("run-1", &w.deps.run).await.unwrap();
    assert_eq!(driven.stop, DriveStop::Done);
    let saw = w.steps_saw.lock().unwrap();
    assert_eq!(saw.len(), 1);
    assert_eq!(saw[0].as_ref().map(|a| a.option_id.as_str()), Some("ana"));
    drop(saw);
    let row = w.store.row("run-1");
    assert_eq!(row.result, json!({ "picked": "ana" }));
    assert!(row.decision.is_none());
}

#[tokio::test]
async fn refuses_a_second_answer_to_a_question_that_has_already_been_answered() {
    let w = parked(&["u-editor"]).await;
    talaria_api::runs::decide::decide(
        DecideArgs {
            run_id: "run-1".into(),
            option_id: "ana".into(),
            note: None,
            by: "u-editor".into(),
            start: Some(false),
        },
        &w.deps,
    )
    .await
    .unwrap();

    let second = talaria_api::runs::decide::decide(
        DecideArgs {
            run_id: "run-1".into(),
            option_id: "ben".into(),
            note: None,
            by: "u-editor".into(),
            start: Some(false),
        },
        &w.deps,
    )
    .await
    .unwrap();

    assert_eq!(
        refused(&second),
        (DecideRefusal::NotAwaiting, Some(RunState::Queued))
    );
    assert_eq!(
        w.store
            .row("run-1")
            .decision
            .as_ref()
            .unwrap()
            .answer
            .as_ref()
            .map(|a| a.option_id.as_str()),
        Some("ana")
    );
}

// ── THE PAUSE PROPERTY ───────────────────────────────────────────────────────
//
// The second of the two things the runs runtime exists to make true, asserted
// as ONE arc rather than in pieces, because the pieces passing individually is
// exactly how a system ends up with a run that parks correctly, announces to
// the wrong people, and resumes without the answer.
//
// Everything here is the real code: the real driver, the real `pause` the
// driver delegates its park to, the real `run_decision_approval` translation,
// the real `may_decide`. Only the store, the lease and the announcer are fakes,
// and the store reimplements the compare-and-set predicates rather than
// accepting what it is told. (The sweep leg — `sweep_unannounced` notifying
// and marking — lives in the approvals module and is tested there; here the
// census entry the sweep would take is proven instead.)

#[tokio::test]
async fn pauses_into_an_approval_tells_the_audience_its_definition_declared_refuses_a_stranger_and_resumes_with_the_answer_in_hand()
 {
    let w = world(&["u-editor"], 1);
    // A fresh run nobody has claimed — so the DRIVER takes the lease and runs
    // the step, and the pause below is the one the step actually asked for.
    let run = w.store.queued_fresh();

    // ── 1. It parks rather than guessing ────────────────────────────────────
    let first = drive(&run.id, &w.deps.run).await.unwrap();
    assert_eq!(first.stop, DriveStop::Awaiting);
    assert_eq!(first.steps, 1);
    let parked = w.store.row(&run.id);
    assert_eq!(parked.state, RunState::Awaiting);
    // Nothing is burning and nothing is held: the lease went back, so the row
    // is not going to look reclaimable while a person thinks about it.
    assert!(parked.lease_owner.is_none());
    assert_eq!(w.lease.held.lock().unwrap().len(), 0);
    // The question is ON THE ROW. Park on one instance, open the approval on
    // your phone: the question has to have survived the process that raised it.
    let d = parked.decision.as_ref().unwrap();
    assert_eq!(d.request.key, "assignee");
    assert!(d.answer.is_none());
    assert_eq!(
        parked.approval_key.as_deref(),
        Some("run:test-decision-run:run-1:assignee")
    );

    // ── 2. The census would tell the right people ───────────────────────────
    // Not the run's owner (u-owner) and not the admins: the definition's
    // authority is the ticket's BOARD, and this is what that resolved to.
    let approval = (w.deps.approval_for)(&parked).expect("the census entry builds");
    assert_eq!(approval.kind, ApprovalKind::RunDecision);
    assert_eq!(
        approval.authority,
        Authority::Board {
            board_id: "board-1".into()
        }
    );
    assert!(approval.title.contains("Two people are assigned"));
    assert_eq!(approval.owner_user_ids, vec!["u-owner".to_string()]);

    // ── 3. A stranger cannot answer it ──────────────────────────────────────
    let res = talaria_api::runs::decide::decide(
        DecideArgs {
            run_id: run.id.clone(),
            option_id: "ana".into(),
            note: None,
            by: "u-stranger".into(),
            start: Some(false),
        },
        &w.deps,
    )
    .await
    .unwrap();
    assert_eq!(
        refused(&res),
        (DecideRefusal::Forbidden, Some(RunState::Awaiting))
    );
    assert_eq!(w.store.row(&run.id).state, RunState::Awaiting);
    assert!(
        w.store
            .row(&run.id)
            .decision
            .as_ref()
            .unwrap()
            .answer
            .is_none()
    );

    // ── 4. Somebody entitled does ───────────────────────────────────────────
    w.steps_saw.lock().unwrap().clear();
    let decided = talaria_api::runs::decide::decide(
        DecideArgs {
            run_id: run.id.clone(),
            option_id: "ana".into(),
            note: Some("Ana has the context".into()),
            by: "u-editor".into(),
            start: Some(false),
        },
        &w.deps,
    )
    .await
    .unwrap();
    assert!(matches!(decided, DecideResult::Decided { .. }));
    let queued = w.store.row(&run.id);
    assert_eq!(queued.state, RunState::Queued);
    assert!(
        queued.lease_owner.is_none(),
        "any instance may pick it up — not just the one that asked"
    );
    let answer = queued.decision.as_ref().unwrap().answer.as_ref().unwrap();
    assert_eq!(answer.option_id, "ana");
    assert_eq!(answer.answered_by.as_deref(), Some("u-editor"));

    // ── 5. And the run resumes WITH the answer ──────────────────────────────
    let second = drive(&run.id, &w.deps.run).await.unwrap();
    assert_eq!(second.stop, DriveStop::Done);
    // The answer reached the STEP, which is the only place it means anything.
    let saw = w.steps_saw.lock().unwrap();
    assert_eq!(
        saw.iter()
            .map(|d| d.as_ref().map(|a| format!(
                "{} by {}",
                a.option_id,
                a.answered_by.clone().unwrap_or_default()
            )))
            .collect::<Vec<_>>(),
        vec![Some("ana by u-editor".to_string())]
    );
    drop(saw);
    let row = w.store.row(&run.id);
    assert_eq!(row.result, json!({ "picked": "ana" }));
    // Cleared by the write that recorded the progress it produced, so a
    // reclaim cannot hand a step an answer it has already acted on.
    assert!(row.decision.is_none());
    assert!(row.approval_key.is_none());
}

// ── the census entry ─────────────────────────────────────────────────────────

#[tokio::test]
async fn describes_a_parked_run_as_an_approval_with_the_authority_its_definition_declared() {
    let w = world(&["u-editor"], 1);
    let run = w.store.running();
    talaria_api::runs::decide::pause(pause_args(), &w.deps.pause_deps())
        .await
        .unwrap();

    let approval = (w.deps.approval_for)(&w.store.row(&run.id)).expect("translates");

    assert_eq!(approval.kind, ApprovalKind::RunDecision);
    // The key on the ROW, not a second derivation of it: the announce marks are
    // keyed on this string, and two spellings would announce the same pause
    // twice.
    assert_eq!(approval.key, "run:test-decision-run:run-1:assignee");
    assert_eq!(
        approval.title,
        "Ticket handover needs a decision: Two people are assigned — who should take this ticket?"
    );
    assert!(approval.detail.contains("Options: Ana · Ben."));
    assert_eq!(approval.href, "/boards/board-1/task-1");
    assert_eq!(
        approval.authority,
        Authority::Board {
            board_id: "board-1".into()
        }
    );
    assert_eq!(approval.owner_user_ids, vec!["u-owner".to_string()]);
}
