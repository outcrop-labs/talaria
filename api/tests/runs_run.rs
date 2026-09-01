// The driver, driven entirely by fakes — the port of run.test.ts. No database,
// no Redis, no real clock: the point of the RunDeps seam is that every rule in
// run.rs (the ordering rule, the clean stops, the attempt count, the exact
// sentences on an error row) is provable without a single service. These run in
// CI alongside the unit tests; only the LIVE store proofs (runs_store.rs) are
// #[ignore]d.
//
// The tape is one interleaved log of everything the driver did — every store
// write, every lease verb, every publish — so ordering claims are asserted
// against what actually happened, in the order it happened.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use talaria_api::runs::define::{
    Authority, DecisionAnswer, DecisionOption, DecisionRequest, RunDecision, RunDefinition, RunRow,
    RunState, RunStepContext, StepResult,
};
use talaria_api::runs::run::{
    DriveStop, EnqueueOptions, LeaseClaim, LeaseRenewal, PauseArgs, PauseOutcome, RunDeps,
    RunLease, cancel_run, drive, enqueue,
};
use talaria_api::runs::store::{
    AnswerOutcome, CancelOutcome, ClaimOutcome, NewRun, RunStore, WriteFailure,
};

// ── The tape ─────────────────────────────────────────────────────────────────

type Tape = Arc<Mutex<Vec<String>>>;

fn say(tape: &Tape, line: impl Into<String>) {
    tape.lock().unwrap().push(line.into());
}

fn tape_of(tape: &Tape) -> Vec<String> {
    tape.lock().unwrap().clone()
}

fn happened_before(tape: &[String], a: &str, b: &str) -> bool {
    match (
        tape.iter().position(|l| l == a),
        tape.iter().position(|l| l == b),
    ) {
        (Some(i), Some(j)) => i < j,
        _ => false,
    }
}

/// `a` (an exact write) before the first publish whose line starts with
/// `b_prefix` — publish lines carry the phase, so they match by prefix.
fn published_after(tape: &[String], a: &str, b_prefix: &str) -> bool {
    match (
        tape.iter().position(|l| l == a),
        tape.iter().position(|l| l.starts_with(b_prefix)),
    ) {
        (Some(i), Some(j)) => i < j,
        _ => false,
    }
}

// ── The fake store ───────────────────────────────────────────────────────────
//
// The CAS predicates are mirrored from the SQL, not invented: a wrong fake here
// would prove the driver against a fiction. The live suite (runs_store.rs)
// proves the same predicates against the real table.

const NOW: i64 = 1_000_000;

struct FakeStore {
    st: Mutex<StoreState>,
    tape: Tape,
}

struct StoreState {
    rows: HashMap<String, RunRow>,
    /// Lease expiry in epoch ms, alongside the row (the row renders it ISO).
    exp: HashMap<String, i64>,
    /// When set, the NEXT `complete` is refused with the given failure — the
    /// row moved under the driver between the step and its terminal write.
    refuse_next_complete: Option<WriteFailure>,
}

impl FakeStore {
    fn new(tape: Tape) -> Arc<Self> {
        Arc::new(Self {
            st: Mutex::new(StoreState {
                rows: HashMap::new(),
                exp: HashMap::new(),
                refuse_next_complete: None,
            }),
            tape,
        })
    }

    /// Put a row in by hand, as a reclaim sweep would have found it.
    fn seed(self: &Arc<Self>, row: RunRow) {
        let mut st = self.st.lock().unwrap();
        if let Some(exp) = row
            .lease_expires_at
            .clone()
            .and_then(|s| s.parse::<i64>().ok())
        {
            st.exp.insert(row.id.clone(), exp);
        }
        st.rows.insert(row.id.clone(), row);
    }

    fn row(self: &Arc<Self>, id: &str) -> RunRow {
        self.st
            .lock()
            .unwrap()
            .rows
            .get(id)
            .cloned()
            .expect("seeded row")
    }

    /// The refusal the `write` half of every guard returns for this row.
    fn refusal(row: &RunRow) -> WriteFailure {
        if row.state == RunState::Cancelled {
            WriteFailure::Cancelled
        } else {
            WriteFailure::LeaseLost { state: row.state }
        }
    }
}

fn new_row(id: &str, kind: &str) -> RunRow {
    RunRow {
        id: id.into(),
        kind: kind.into(),
        owner_user_id: Some("user-1".into()),
        subject_type: None,
        subject_id: None,
        state: RunState::Queued,
        phase: "starting".into(),
        checkpoint: Value::Null,
        input: json!({}),
        result: Value::Null,
        error: None,
        attempt: 0,
        lease_owner: None,
        lease_expires_at: None,
        approval_key: None,
        decision: None,
        created_at: "t0".into(),
        updated_at: "t0".into(),
        started_at: None,
        finished_at: None,
    }
}

impl RunStore for FakeStore {
    fn insert<'a>(
        &'a self,
        row: NewRun,
    ) -> futures_util::future::BoxFuture<'a, Result<RunRow, sqlx::Error>> {
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
                created_at: "t0".into(),
                updated_at: "t0".into(),
                id: row.id,
                kind: row.kind,
                owner_user_id: row.owner_user_id,
                subject_type: row.subject_type,
                subject_id: row.subject_id,
                input: row.input,
                phase: row.phase,
            };
            say(&self.tape, "db:insert");
            self.st
                .lock()
                .unwrap()
                .rows
                .insert(full.id.clone(), full.clone());
            Ok(full)
        })
    }

    fn get<'a>(
        &'a self,
        id: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<Option<RunRow>, sqlx::Error>> {
        Box::pin(async move {
            let st = self.st.lock().unwrap();
            Ok(st.rows.get(id).cloned())
        })
    }

    fn claim<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        lease_ms: i64,
    ) -> futures_util::future::BoxFuture<'a, Result<ClaimOutcome, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(mut row) = st.rows.get(id).cloned() else {
                return Ok(ClaimOutcome::Missing);
            };
            if !matches!(row.state, RunState::Queued | RunState::Running) {
                return Ok(ClaimOutcome::NotRunnable { state: row.state });
            }
            let held_until = row
                .lease_owner
                .as_deref()
                .and_then(|_| st.exp.get(id).copied())
                .filter(|exp| *exp > NOW);
            if let Some(exp) = held_until {
                return Ok(ClaimOutcome::Taken {
                    state: row.state,
                    until: Some(exp.to_string()),
                });
            }
            let reclaimed = row.state == RunState::Running; // the CTE's `case when prev`
            if reclaimed {
                row.attempt += 1;
            }
            row.state = RunState::Running;
            row.lease_owner = Some(token.to_string());
            row.lease_expires_at = Some((NOW + lease_ms).to_string());
            if row.started_at.is_none() {
                row.started_at = Some("t1".into());
            }
            st.exp.insert(id.to_string(), NOW + lease_ms);
            st.rows.insert(id.to_string(), row.clone());
            say(&self.tape, "db:claim");
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
    ) -> futures_util::future::BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(row) = st.rows.get_mut(id) else {
                return Ok(Err(WriteFailure::Missing));
            };
            if row.state == RunState::Cancelled {
                return Ok(Err(WriteFailure::Cancelled));
            }
            if row.lease_owner.as_deref() != Some(token) {
                return Ok(Err(WriteFailure::LeaseLost { state: row.state }));
            }
            row.lease_expires_at = Some((NOW + lease_ms).to_string());
            st.exp.insert(id.to_string(), NOW + lease_ms);
            Ok(Ok(()))
        })
    }

    fn checkpoint<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        checkpoint: Value,
        phase: String,
        clear_decision: bool,
    ) -> futures_util::future::BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(row) = st.rows.get_mut(id) else {
                return Ok(Err(WriteFailure::Missing));
            };
            if row.state == RunState::Cancelled {
                return Ok(Err(WriteFailure::Cancelled));
            }
            if row.lease_owner.as_deref() != Some(token) {
                return Ok(Err(Self::refusal(row)));
            }
            row.checkpoint = checkpoint;
            row.phase = phase;
            if clear_decision {
                // The SQL nulls the whole decision and the key with it.
                row.decision = None;
                row.approval_key = None;
            }
            say(
                &self.tape,
                if clear_decision {
                    "db:checkpoint:clear"
                } else {
                    "db:checkpoint"
                },
            );
            Ok(Ok(()))
        })
    }

    fn phase<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        phase: String,
    ) -> futures_util::future::BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(row) = st.rows.get_mut(id) else {
                return Ok(Err(WriteFailure::Missing));
            };
            if row.state == RunState::Cancelled {
                return Ok(Err(WriteFailure::Cancelled));
            }
            if row.lease_owner.as_deref() != Some(token) {
                return Ok(Err(Self::refusal(row)));
            }
            row.phase = phase;
            say(&self.tape, "db:phase");
            Ok(Ok(()))
        })
    }

    fn complete<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        result: Value,
    ) -> futures_util::future::BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            if let Some(f) = st.refuse_next_complete.take() {
                say(&self.tape, "db:complete:refused");
                return Ok(Err(f));
            }
            let Some(row) = st.rows.get_mut(id) else {
                return Ok(Err(WriteFailure::Missing));
            };
            if row.state == RunState::Cancelled {
                return Ok(Err(WriteFailure::Cancelled));
            }
            if row.lease_owner.as_deref() != Some(token) {
                return Ok(Err(Self::refusal(row)));
            }
            row.state = RunState::Done;
            row.result = result;
            row.decision = None;
            row.approval_key = None;
            row.finished_at = Some("t9".into());
            row.lease_owner = None;
            row.lease_expires_at = None;
            st.exp.remove(id);
            say(&self.tape, "db:complete");
            Ok(Ok(()))
        })
    }

    fn fail<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        error: String,
    ) -> futures_util::future::BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(row) = st.rows.get_mut(id) else {
                return Ok(Err(WriteFailure::Missing));
            };
            if row.state == RunState::Cancelled {
                return Ok(Err(WriteFailure::Cancelled));
            }
            if row.lease_owner.as_deref() != Some(token) {
                return Ok(Err(Self::refusal(row)));
            }
            row.state = RunState::Error;
            row.error = Some(error);
            row.finished_at = Some("t9".into());
            row.lease_owner = None;
            row.lease_expires_at = None;
            st.exp.remove(id);
            say(&self.tape, "db:fail");
            Ok(Ok(()))
        })
    }

    fn park<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        decision: RunDecision,
        approval_key: String,
        phase: String,
    ) -> futures_util::future::BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(row) = st.rows.get_mut(id) else {
                return Ok(Err(WriteFailure::Missing));
            };
            if row.state == RunState::Cancelled {
                return Ok(Err(WriteFailure::Cancelled));
            }
            if row.lease_owner.as_deref() != Some(token) {
                return Ok(Err(Self::refusal(row)));
            }
            row.state = RunState::Awaiting;
            row.decision = Some(decision);
            row.approval_key = Some(approval_key);
            row.phase = phase;
            row.lease_owner = None; // the row is nobody's while it waits
            row.lease_expires_at = None;
            st.exp.remove(id);
            say(&self.tape, "db:park");
            Ok(Ok(()))
        })
    }

    fn defer<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
        until_ms: i64,
        reason: String,
    ) -> futures_util::future::BoxFuture<'a, Result<Result<(), WriteFailure>, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(row) = st.rows.get_mut(id) else {
                return Ok(Err(WriteFailure::Missing));
            };
            if row.state == RunState::Cancelled {
                return Ok(Err(WriteFailure::Cancelled));
            }
            if row.lease_owner.as_deref() != Some(token) {
                return Ok(Err(Self::refusal(row)));
            }
            row.state = RunState::Queued;
            row.phase = reason;
            // The lease stamp IS the wait.
            row.lease_owner = Some(token.to_string());
            row.lease_expires_at = Some(until_ms.to_string());
            st.exp.insert(id.to_string(), until_ms);
            say(&self.tape, "db:defer");
            Ok(Ok(()))
        })
    }

    fn release<'a>(
        &'a self,
        id: &'a str,
        token: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<(), sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            if let Some(row) = st.rows.get_mut(id)
                && row.lease_owner.as_deref() == Some(token)
            {
                row.lease_owner = None;
                row.lease_expires_at = None;
                st.exp.remove(id);
            }
            say(&self.tape, "db:release");
            Ok(())
        })
    }

    fn answer<'a>(
        &'a self,
        id: &'a str,
        answer: DecisionAnswer,
    ) -> futures_util::future::BoxFuture<'a, Result<AnswerOutcome, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(mut row) = st.rows.get(id).cloned() else {
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
            row.decision.as_mut().unwrap().answer = Some(answer);
            row.state = RunState::Queued;
            // The SQL keeps the approval key: the census re-derives nothing,
            // and `run_decision_approval` already returns None off the state.
            row.lease_owner = None;
            row.lease_expires_at = None;
            st.exp.remove(id);
            st.rows.insert(id.to_string(), row.clone());
            say(&self.tape, "db:answer");
            Ok(AnswerOutcome::Answered(Box::new(row)))
        })
    }

    fn cancel<'a>(
        &'a self,
        id: &'a str,
        reason: Option<String>,
    ) -> futures_util::future::BoxFuture<'a, Result<CancelOutcome, sqlx::Error>> {
        Box::pin(async move {
            let mut st = self.st.lock().unwrap();
            let Some(row) = st.rows.get_mut(id) else {
                return Ok(CancelOutcome::Missing);
            };
            if !matches!(
                row.state,
                RunState::Queued | RunState::Running | RunState::Awaiting
            ) {
                return Ok(CancelOutcome::Terminal { state: row.state });
            }
            row.state = RunState::Cancelled;
            row.error = reason;
            row.finished_at = Some("t8".into());
            row.lease_owner = None;
            row.lease_expires_at = None;
            st.exp.remove(id);
            say(&self.tape, "db:cancel");
            Ok(CancelOutcome::Cancelled {
                state: RunState::Cancelled,
            })
        })
    }

    fn due<'a>(
        &'a self,
        limit: i64,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
        Box::pin(async move {
            let st = self.st.lock().unwrap();
            Ok(st
                .rows
                .values()
                .filter(|r| matches!(r.state, RunState::Queued | RunState::Running))
                .filter(|r| r.lease_owner.is_none() || st.exp.get(&r.id).is_none_or(|e| *e <= NOW))
                .take(limit.max(0) as usize)
                .cloned()
                .collect())
        })
    }

    fn active_for<'a>(
        &'a self,
        user_id: &'a str,
        limit: Option<i64>,
    ) -> futures_util::future::BoxFuture<'a, Result<Vec<RunRow>, sqlx::Error>> {
        Box::pin(async move {
            let st = self.st.lock().unwrap();
            Ok(st
                .rows
                .values()
                .filter(|r| {
                    r.owner_user_id.as_deref() == Some(user_id)
                        && matches!(
                            r.state,
                            RunState::Queued | RunState::Running | RunState::Awaiting
                        )
                })
                .take(limit.unwrap_or(50).max(1) as usize)
                .cloned()
                .collect())
        })
    }
}

// ── The fake lease ───────────────────────────────────────────────────────────

enum ScriptedAcquire {
    Ok,
    Busy,
    Blocked,
}

struct FakeLease {
    tape: Tape,
    acquire: Mutex<ScriptedAcquire>,
    /// Popped per renew call; empty means every beat is ok.
    renews: Mutex<VecDeque<LeaseRenewal>>,
    counter: std::sync::atomic::AtomicUsize,
}

impl FakeLease {
    fn new(tape: Tape) -> Arc<Self> {
        Arc::new(Self {
            tape,
            acquire: Mutex::new(ScriptedAcquire::Ok),
            renews: Mutex::new(VecDeque::new()),
            counter: std::sync::atomic::AtomicUsize::new(0),
        })
    }
}

impl RunLease for FakeLease {
    fn acquire<'a>(
        &'a self,
        _run_id: &'a str,
        _step_ms: i64,
    ) -> futures_util::future::BoxFuture<'a, LeaseClaim> {
        Box::pin(async move {
            let n = self
                .counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            say(&self.tape, "lease:acquire");
            match &*self.acquire.lock().unwrap() {
                ScriptedAcquire::Ok => LeaseClaim::Claimed {
                    token: format!("tok-{n}"),
                },
                ScriptedAcquire::Busy => LeaseClaim::Busy,
                ScriptedAcquire::Blocked => LeaseClaim::Blocked {
                    error: "connection dropped".into(),
                },
            }
        })
    }

    fn renew<'a>(
        &'a self,
        _run_id: &'a str,
        _token: &'a str,
        _step_ms: i64,
    ) -> futures_util::future::BoxFuture<'a, LeaseRenewal> {
        Box::pin(async move {
            let r = self
                .renews
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(LeaseRenewal::Ok);
            say(&self.tape, format!("lease:renew:{r:?}"));
            r
        })
    }

    fn release<'a>(
        &'a self,
        _run_id: &'a str,
        _token: &'a str,
    ) -> futures_util::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            say(&self.tape, "lease:release");
        })
    }
}

// ── The fixture ──────────────────────────────────────────────────────────────

struct Fixture {
    deps: RunDeps,
    store: Arc<FakeStore>,
    lease: Arc<FakeLease>,
    tape: Tape,
    pause_calls: Arc<Mutex<Vec<String>>>,
    pause_answers: Arc<Mutex<VecDeque<PauseOutcome>>>,
}

impl Fixture {
    fn with(def: RunDefinition) -> Self {
        let tape: Tape = Arc::new(Mutex::new(Vec::new()));
        let store = FakeStore::new(tape.clone());
        let lease = FakeLease::new(tape.clone());
        let pause_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let pause_answers: Arc<Mutex<VecDeque<PauseOutcome>>> =
            Arc::new(Mutex::new(VecDeque::new()));

        let def = Arc::new(def);
        let defs: HashMap<String, Arc<RunDefinition>> =
            HashMap::from([(def.kind.clone(), def.clone())]);

        let publish_tape = tape.clone();
        let pause_seen = pause_calls.clone();
        let pause_next = pause_answers.clone();

        let deps = RunDeps {
            store: store.clone(),
            lease: lease.clone(),
            publish: Arc::new(move |ev, owner| {
                say(
                    &publish_tape,
                    format!(
                        "pub:{}:{}",
                        serde_json::to_string(&ev.state).unwrap(),
                        ev.phase
                    ),
                );
                let _ = owner;
            }),
            pause: Arc::new(move |args: PauseArgs| {
                Box::pin({
                    let seen = pause_seen.clone();
                    let next = pause_next.clone();
                    async move {
                        seen.lock()
                            .unwrap()
                            .push(format!("{}|{}", args.token, args.question.key));
                        next.lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or(PauseOutcome::Parked {
                                approval_key: "approval:test".into(),
                                announced: 1,
                            })
                    }
                })
            }),
            definition_for: Arc::new(move |kind| defs.get(kind).cloned()),
            now: Arc::new(|| NOW),
            new_id: Arc::new(|| "generated-id".into()),
        };

        Self {
            deps,
            store,
            lease,
            tape,
            pause_calls,
            pause_answers,
        }
    }

    /// Refuse the pause — the run was cancelled or taken while asking.
    fn refuse_pause(&self) {
        self.pause_answers
            .lock()
            .unwrap()
            .push_back(PauseOutcome::Refused {
                reason: WriteFailure::Cancelled,
                state: Some(RunState::Cancelled),
            });
    }
}

fn definition(
    kind: &str,
    max_step_ms: u64,
    step: talaria_api::runs::define::StepFn,
) -> RunDefinition {
    RunDefinition {
        kind: kind.into(),
        label: kind.into(),
        step,
        audience: Arc::new(|_| Authority::Nobody),
        max_step_ms,
        max_attempts: 3,
    }
}

/// A step that walks n `next`s then finishes — the happy path, countable.
fn walking_step(n: u64, final_phase: Option<&'static str>) -> talaria_api::runs::define::StepFn {
    Arc::new(move |ctx: RunStepContext| {
        Box::pin(async move {
            let i = ctx.checkpoint.as_u64().unwrap_or(0);
            if i >= n {
                (ctx.log)("finishing".into());
                return Ok(StepResult::Done {
                    result: json!({"walked": n}),
                });
            }
            (ctx.log)(format!("step {i}"));
            Ok(StepResult::Next {
                checkpoint: json!(i + 1),
                phase: final_phase.map(|p| p.to_string()),
            })
        })
    })
}

// ── The drives ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn walks_next_to_done_persisting_before_every_publish() {
    let fx = Fixture::with(definition("walk", 5_000, walking_step(3, None)));
    let id = "run-walk";
    fx.store.seed(new_row(id, "walk"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Done);
    assert_eq!(
        res.steps, 4,
        "three nexts plus the entry that sees the last checkpoint"
    );
    assert_eq!(fx.store.row(id).state, RunState::Done);
    assert_eq!(fx.store.row(id).result, json!({"walked": 3}));

    // THE ORDERING RULE, on the tape: each publish trails its write — a phase
    // line is persisted before it is published, and the terminal publish
    // trails the terminal write.
    let tape = tape_of(&fx.tape);
    assert_eq!(tape.iter().filter(|l| **l == "db:checkpoint").count(), 3);
    for i in 0..3 {
        assert!(
            happened_before(&tape, "db:phase", &format!("pub:\"running\":step {i}")),
            "phase line {i} must persist before it publishes: {tape:?}"
        );
    }
    assert!(happened_before(&tape, "db:phase", "db:complete"));
    assert!(happened_before(
        &tape,
        "db:complete",
        "pub:\"done\":finishing"
    ));
    // The drive handed both leases back.
    assert!(tape.iter().any(|l| l == "lease:release"));
    assert!(tape.iter().any(|l| l == "db:release"));
}

#[tokio::test]
async fn a_thrown_step_files_the_first_line_on_an_error_row() {
    let fx = Fixture::with(definition(
        "throw",
        5_000,
        Arc::new(|_| Box::pin(async { Err("kapow\n  at some step (foo.ts:1)".to_string()) })),
    ));
    let id = "run-throw";
    fx.store.seed(new_row(id, "throw"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Error);
    assert_eq!(res.steps, 1);
    assert_eq!(fx.store.row(id).state, RunState::Error);
    assert_eq!(fx.store.row(id).error.as_deref(), Some("kapow"));
    assert_eq!(res.error.as_deref(), Some("kapow"));
    let tape = tape_of(&fx.tape);
    assert!(published_after(&tape, "db:fail", "pub:\"error\""));
}

#[tokio::test]
async fn decide_parks_through_pause_and_awaits() {
    let fx = Fixture::with(definition(
        "ask",
        5_000,
        Arc::new(|_| {
            Box::pin(async {
                Ok(StepResult::Decide {
                    question: DecisionRequest {
                        key: "which".into(),
                        question: "Which source?".into(),
                        detail: None,
                        options: vec![DecisionOption {
                            id: "a".into(),
                            label: "A".into(),
                            detail: None,
                        }],
                        href: None,
                    },
                })
            })
        }),
    ));
    let id = "run-ask";
    fx.store.seed(new_row(id, "ask"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Awaiting);
    assert_eq!(res.steps, 1);
    // The pause got the driver's token and the question's key.
    let calls = fx.pause_calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].ends_with("|which"));
    assert!(calls[0].starts_with("tok-"));
    // THE QUESTION DOES NOT GO ON THE WIRE from the driver: the park (and its
    // announce) belongs to pause; here only the state transition was said.
    assert!(!tape_of(&fx.tape).iter().any(|l| l.contains("Which source")));
}

#[tokio::test]
async fn a_refused_pause_is_not_an_error() {
    let fx = Fixture::with(definition(
        "refused-ask",
        5_000,
        Arc::new(|_| {
            Box::pin(async {
                Ok(StepResult::Decide {
                    question: DecisionRequest {
                        key: "which".into(),
                        question: "Which?".into(),
                        detail: None,
                        options: vec![],
                        href: None,
                    },
                })
            })
        }),
    ));
    fx.refuse_pause();
    let id = "run-refused-ask";
    fx.store.seed(new_row(id, "refused-ask"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Cancelled);
    // Nothing was failed; there is no error row for "a person did not answer".
    assert!(!tape_of(&fx.tape).iter().any(|l| l == "db:fail"));
}

#[tokio::test]
async fn retry_defers_and_holds_the_lease() {
    let fx = Fixture::with(definition(
        "retry",
        5_000,
        Arc::new(|_| {
            Box::pin(async {
                Ok(StepResult::Retry {
                    after: Duration::from_secs(30),
                    reason: "rate limited".into(),
                })
            })
        }),
    ));
    let id = "run-retry";
    fx.store.seed(new_row(id, "retry"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Deferred);
    assert_eq!(res.retry_after_ms, Some(30_000));
    let row = fx.store.row(id);
    assert_eq!(row.state, RunState::Queued);
    assert_eq!(row.phase, "rate limited");
    // No attempt consumed, no notification filed.
    assert_eq!(row.attempt, 0);
    let tape = tape_of(&fx.tape);
    assert!(tape.iter().any(|l| l == "db:defer"));
    assert!(happened_before(
        &tape,
        "db:defer",
        "pub:\"queued\":rate limited"
    ));
    // THE LEASE IS THE WAIT: the deferral is held by renewing, and the lease
    // is NOT released — the next sweep finds the row takeable only after it.
    assert!(tape.iter().any(|l| l.starts_with("lease:renew:")));
    assert!(!tape.iter().any(|l| l == "lease:release"));
    assert!(!tape.iter().any(|l| l == "db:release"));
}

#[tokio::test]
async fn busy_is_not_an_error() {
    let fx = Fixture::with(definition("busy", 5_000, walking_step(1, None)));
    let id = "run-busy";
    fx.store.seed(new_row(id, "busy"));
    *fx.lease.acquire.lock().unwrap() = ScriptedAcquire::Busy;

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Busy);
    // One Redis round trip and nothing else: no claim, no writes, no publish.
    assert_eq!(tape_of(&fx.tape), vec!["lease:acquire".to_string()]);
}

#[tokio::test]
async fn blocked_leaves_the_row_alone() {
    let fx = Fixture::with(definition("blocked", 5_000, walking_step(1, None)));
    let id = "run-blocked";
    fx.store.seed(new_row(id, "blocked"));
    *fx.lease.acquire.lock().unwrap() = ScriptedAcquire::Blocked;

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Blocked);
    assert_eq!(res.error.as_deref(), Some("connection dropped"));
    // Fail closed WITHOUT failing the run: the row is exactly as it was found.
    assert_eq!(tape_of(&fx.tape), vec!["lease:acquire".to_string()]);
    let row = fx.store.row(id);
    assert_eq!(row.state, RunState::Queued);
    assert!(row.error.is_none());
}

#[tokio::test]
async fn a_lease_lost_mid_step_is_a_clean_stop() {
    // The step runs long (longer than the deadline); the FIRST renewal beat
    // comes back Lost. The drive must stop cleanly — no fail write, ever.
    let fx = Fixture::with(definition(
        "lost",
        8_000,
        Arc::new(|_ctx| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                unreachable!("the driver must not let a lost-lease step finish")
            })
        }),
    ));
    fx.lease.loses_the_next_renew();
    let id = "run-lost";
    fx.store.seed(new_row(id, "lost"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::LeaseLost);
    let tape = tape_of(&fx.tape);
    assert!(tape.iter().any(|l| l == "lease:renew:Lost"));
    assert!(!tape.iter().any(|l| l == "db:fail"));
    // The row is still running, for whoever took it — untouched.
    assert_eq!(fx.store.row(id).state, RunState::Running);
    assert!(tape.iter().any(|l| l == "lease:release"));
}

#[tokio::test]
async fn cancellation_is_honored_at_the_boundary() {
    // The step itself cancels the run (as any instance may) before returning
    // progress; the next boundary must see it and stop — without applying the
    // result or filing anything.
    let id = "run-cancel";
    let fx = {
        let store_holder: Arc<Mutex<Option<Arc<FakeStore>>>> = Arc::new(Mutex::new(None));
        let sh = store_holder.clone();
        let step_store = store_holder.clone();
        let step_id = id.to_string();
        let fx = Fixture::with(definition(
            "cancel-mid",
            5_000,
            Arc::new(move |ctx| {
                let store = step_store.clone();
                let run_id = step_id.clone();
                Box::pin(async move {
                    if ctx.checkpoint.is_null() {
                        let store = store.lock().unwrap().clone().unwrap();
                        store
                            .cancel(&run_id, Some("changed my mind".into()))
                            .await
                            .unwrap();
                        return Ok(StepResult::Next {
                            checkpoint: json!(1),
                            phase: None,
                        });
                    }
                    Ok(StepResult::Done {
                        result: Value::Null,
                    })
                })
            }),
        ));
        *store_holder.lock().unwrap() = Some(fx.store.clone());
        let _ = sh;
        fx
    };
    fx.store.seed(new_row(id, "cancel-mid"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Cancelled);
    assert_eq!(res.steps, 1);
    let row = fx.store.row(id);
    assert_eq!(row.state, RunState::Cancelled);
    assert_eq!(row.error.as_deref(), Some("changed my mind"));
    // The step's result was discarded, not applied.
    assert!(row.checkpoint.is_null());
    let tape = tape_of(&fx.tape);
    assert!(!tape.iter().any(|l| l == "db:fail"));
}

#[tokio::test]
async fn exhausted_attempts_files_the_exact_message() {
    let fx = Fixture::with(definition("exhaust", 5_000, walking_step(1, None)));
    let id = "run-exhaust";
    let mut row = new_row(id, "exhaust");
    row.attempt = 3; // three drivers died holding this run
    fx.store.seed(row);

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Exhausted);
    assert_eq!(res.steps, 0); // the step is never entered
    assert_eq!(
        fx.store.row(id).error.as_deref(),
        Some(
            "run gave up after 3 attempt(s): each driver that took it stopped without finishing \
             or checkpointing"
        )
    );
    let tape = tape_of(&fx.tape);
    assert!(published_after(&tape, "db:fail", "pub:\"error\""));
}

#[tokio::test]
async fn a_step_over_budget_is_an_error_row_never_retried() {
    let fx = Fixture::with(definition(
        "overdue",
        50, // the definition's own statement of one unit of progress
        Arc::new(|_ctx| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                unreachable!("the deadline must win")
            })
        }),
    ));
    let id = "run-overdue";
    fx.store.seed(new_row(id, "overdue"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Error);
    assert_eq!(
        fx.store.row(id).error.as_deref(),
        Some(
            "step exceeded maxStepMs (50ms) at phase \"starting\". The step may still be \
             running; it will not be retried."
        )
    );
}

#[tokio::test]
async fn a_kind_this_process_does_not_know_is_left_alone() {
    // definition_for finds nothing: the row is a newer deploy's kind. Not an
    // error, not a write — this instance is simply not the one to drive it.
    let fx = Fixture::with(definition("known", 5_000, walking_step(1, None)));
    let id = "run-unknown";
    fx.store.seed(new_row(id, "some-future-kind"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::NoDefinition);
    assert_eq!(fx.store.row(id).state, RunState::Queued);
    assert!(tape_of(&fx.tape).is_empty());
}

#[tokio::test]
async fn missing_is_missing() {
    let fx = Fixture::with(definition("gone", 5_000, walking_step(1, None)));
    let res = drive("no-such-run", &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Missing);
    assert!(tape_of(&fx.tape).is_empty());
}

#[tokio::test]
async fn a_refused_terminal_write_is_a_clean_stop() {
    // The row moved under the driver between the step and its `done`: another
    // instance finished it. That is a handover, not a failure.
    let fx = Fixture::with(definition(
        "raced-done",
        5_000,
        Arc::new(|_| {
            Box::pin(async {
                Ok(StepResult::Done {
                    result: Value::Null,
                })
            })
        }),
    ));
    fx.store.st.lock().unwrap().refuse_next_complete = Some(WriteFailure::LeaseLost {
        state: RunState::Running,
    });
    let id = "run-raced-done";
    fx.store.seed(new_row(id, "raced-done"));

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::LeaseLost);
    let tape = tape_of(&fx.tape);
    assert!(tape.iter().any(|l| l == "db:complete:refused"));
    assert!(!tape.iter().any(|l| l == "db:fail"));
    assert!(!tape.iter().any(|l| l.starts_with("pub:\"done\"")));
    // And the lease was still handed back.
    assert!(tape.iter().any(|l| l == "lease:release"));
}

#[tokio::test]
async fn the_answer_clears_in_the_same_write_as_the_checkpoint() {
    // A run parked, answered, resumed: the consuming step's ONE checkpoint
    // write carries the clear. Two writes would be a window where a reclaim
    // hands the next step an answer it already acted on.
    let seen_answer: Arc<Mutex<Option<DecisionAnswer>>> = Arc::new(Mutex::new(None));
    let sa = seen_answer.clone();
    let fx = Fixture::with(definition(
        "consume",
        5_000,
        Arc::new(move |ctx| {
            let seen = sa.clone();
            Box::pin(async move {
                if ctx.checkpoint.is_null() {
                    *seen.lock().unwrap() = ctx.decision.clone();
                    return Ok(StepResult::Next {
                        checkpoint: json!({"acted": true}),
                        phase: None,
                    });
                }
                Ok(StepResult::Done {
                    result: Value::Null,
                })
            })
        }),
    ));
    let id = "run-consume";
    let mut row = new_row(id, "consume");
    row.decision = Some(RunDecision {
        request: DecisionRequest {
            key: "which".into(),
            question: "Which?".into(),
            detail: None,
            options: vec![],
            href: None,
        },
        answer: Some(DecisionAnswer {
            key: "which".into(),
            option_id: "a".into(),
            note: None,
            answered_by: Some("user-1".into()),
            answered_at: "t7".into(),
        }),
    });
    fx.store.seed(row);

    let res = drive(id, &fx.deps).await.unwrap();
    assert_eq!(res.stop, DriveStop::Done);
    // The step SAW the answer…
    assert_eq!(seen_answer.lock().unwrap().as_ref().unwrap().option_id, "a");
    // …and exactly one checkpoint write happened, carrying the clear.
    let tape = tape_of(&fx.tape);
    assert_eq!(
        tape.iter().filter(|l| **l == "db:checkpoint:clear").count(),
        1
    );
    assert!(!tape.iter().any(|l| l == "db:checkpoint"));
    // …and the SQL nulls the whole decision with it, not just the answer — a
    // reclaim cannot hand a step an answer it has already acted on, and there
    // is no half-cleared question left on the row either.
    let row = fx.store.row(id);
    assert!(row.decision.is_none());
    assert!(row.approval_key.is_none());
}

#[tokio::test]
async fn enqueue_writes_before_it_publishes_and_only_drives_when_asked() {
    fn make_def() -> RunDefinition {
        definition("enqueue-me", 5_000, walking_step(1, None))
    }
    let def = Arc::new(make_def());
    let fx = Fixture::with(make_def());

    let run = enqueue(
        &def,
        json!({"x": 1}),
        EnqueueOptions {
            owner_user_id: Some("user-1".into()),
            phase: Some("queued".into()),
            start: Some(false), // deterministic: no detached drive in this test
            ..Default::default()
        },
        &fx.deps,
    )
    .await
    .unwrap();

    assert_eq!(run.state, RunState::Queued);
    assert_eq!(fx.store.row(&run.id).input, json!({"x": 1}));
    assert_eq!(fx.store.row(&run.id).phase, "queued");
    // THE ORDERING RULE, first instance: the row exists before it is said to.
    assert_eq!(
        tape_of(&fx.tape),
        vec!["db:insert".to_string(), "pub:\"queued\":queued".to_string()]
    );
}

#[tokio::test]
async fn cancel_run_publishes_the_state_it_persisted() {
    let fx = Fixture::with(definition("cancellable", 5_000, walking_step(9, None)));
    let id = "run-cancel-api";
    fx.store.seed(new_row(id, "cancellable"));

    match cancel_run(id, Some("not needed".into()), &fx.deps)
        .await
        .unwrap()
    {
        CancelOutcome::Cancelled { state } => assert_eq!(state, RunState::Cancelled),
        other => panic!("expected cancelled, got {other:?}"),
    }
    let tape = tape_of(&fx.tape);
    assert!(published_after(&tape, "db:cancel", "pub:\"cancelled\""));
    // A second cancel says terminal, not missing — the row still exists.
    assert!(matches!(
        cancel_run(id, None, &fx.deps).await.unwrap(),
        CancelOutcome::Terminal { .. }
    ));
}

#[tokio::test]
async fn active_runs_lists_what_this_person_has_in_flight() {
    let fx = Fixture::with(definition("active", 5_000, walking_step(1, None)));
    let mut mine = new_row("run-mine", "active");
    mine.owner_user_id = Some("me".into());
    let mut done = new_row("run-done", "active");
    done.owner_user_id = Some("me".into());
    done.state = RunState::Done;
    let mut theirs = new_row("run-theirs", "active");
    theirs.owner_user_id = Some("you".into());
    fx.store.seed(mine);
    fx.store.seed(done);
    fx.store.seed(theirs);

    let active = talaria_api::runs::run::active_runs("me", &fx.deps)
        .await
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "run-mine");
}

// A tiny extension point so the lost-lease test reads as intent.
impl FakeLease {
    fn loses_the_next_renew(&self) {
        self.renews.lock().unwrap().push_back(LeaseRenewal::Lost);
    }
}
