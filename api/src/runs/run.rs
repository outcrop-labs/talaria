// THE driver. One chokepoint that turns a `RunDefinition` into a durable run:
// it takes the lease, loops `step()`, persists every checkpoint, parks on a
// human decision, and gives the lease back — and it is the only code in the
// runs system that writes a state transition.
//
// WHAT IT GUARANTEES, and what it deliberately does not:
//
//   IT SURVIVES THE PROCESS. Nothing about a run lives in this process except
//   the lease token and the current step's future. A restart, a deploy, a
//   container paused past its lease: the row is still there, still `running`,
//   with its last checkpoint, and the next sweep re-enters `step()` with it.
//
//   IT IS AT-LEAST-ONCE, and this is the risk every caller must hold. A
//   reclaimed run RE-ENTERS `step()` with the last PERSISTED checkpoint. A
//   step that ran and did not persist runs AGAIN — if it archived a chat,
//   sent a DM, opened a PR or billed a model call, that happens twice. The
//   driver's own ordering rule (below) makes the window as small as a
//   database write, and no smaller.
//
//   A LOST LEASE IS A CLEAN STOP. Not an error, not a failure, not a
//   notification: another instance owns this run now, and the correct behavior
//   is to stop touching it and say so at log level.
//
//   CANCELLATION IS HONORED BY ANY INSTANCE. The driver re-reads the row at
//   every step boundary and every write requires `state = 'running'`, so a
//   cancel issued on a different instance stops this one at the next boundary.
//
//   AN ERRED STEP IS AN ERROR ROW WITH THE MESSAGE ON IT (an `Err` from the
//   step future). Never swallowed, never a silent
//   early return, never a state left at `running` for a sweep to reinterpret.
//
// TESTABILITY IS A DESIGN CONSTRAINT. Every edge to the outside world — the
// store, the lease, the publish, the park, the clock, the id generator, the
// definition lookup — is a field on `RunDeps`. The tests (tests/runs_run.rs)
// drive the whole runner with no database, no Redis and no clock. The real
// assembly is `real_run_deps` in runs/mod.rs.

use super::define::{
    DecisionAnswer, DecisionRequest, RunDefinition, RunRow, RunState, RunStepContext, StepResult,
    is_drivable, run_definition as registry_definition,
};
use super::lease::{
    LeaseResult, LeaseToken, RedisLeases, RunClaim, acquire_run_lease, release_run_lease,
    renew_lease, run_lease_key,
};
use super::store::{CancelOutcome, ClaimOutcome, NewRun, RunStore, WriteFailure};
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

const LOG: &str = "[runs]";

// ── The lease, as the driver needs it ────────────────────────────────────────
//
// The mutual exclusion itself lives in runs/lease.rs — one primitive, shared
// with the scheduler, with the compare-and-set scripts and the token format in
// one place. This trait is the narrow seam the DRIVER holds it through: three
// verbs, keyed by run id, with the token as a plain string because that same
// string is also the row's `lease_owner`. One identity, in Redis and in
// Postgres, is what makes "is this run still mine" answerable from either side.

/// `lost` and `unavailable` are kept apart on purpose: one means another
/// instance owns the run now, the other means we could not say. Both stop this
/// driver; they are different sentences in the log, and collapsing them is how
/// a Redis blip gets read as a handover for the rest of the incident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseRenewal {
    Ok,
    Lost,
    Unavailable,
}

#[derive(Debug, Clone)]
pub enum LeaseClaim {
    Claimed {
        token: String,
    },
    /// Another instance is stepping this run right now. Not an error.
    Busy,
    /// Redis could not be asked, so this process cannot know whether it is
    /// alone. LEAVE THE ROW ALONE: do not step it, and above all do not mark
    /// it failed. The checkpoint is durable and a later sweep will claim it.
    Blocked {
        error: String,
    },
}

pub trait RunLease: Send + Sync {
    /// Claim the run for ONE STEP's worth of time.
    fn acquire<'a>(&'a self, run_id: &'a str, step_ms: i64) -> BoxFuture<'a, LeaseClaim>;
    fn renew<'a>(
        &'a self,
        run_id: &'a str,
        token: &'a str,
        step_ms: i64,
    ) -> BoxFuture<'a, LeaseRenewal>;
    /// Compare-and-delete. A lease released by anyone but its owner would be a
    /// lease that a slow driver deletes out from under its successor.
    fn release<'a>(&'a self, run_id: &'a str, token: &'a str) -> BoxFuture<'a, ()>;
}

/// The real lease over the shared primitive. `ConnectionManager` clones per
/// call, so `&self` holds.
pub struct RedisRunLease {
    conn: redis::aio::ConnectionManager,
}

impl RedisRunLease {
    pub fn new(conn: redis::aio::ConnectionManager) -> Self {
        Self { conn }
    }
}

impl RunLease for RedisRunLease {
    fn acquire<'a>(&'a self, run_id: &'a str, step_ms: i64) -> BoxFuture<'a, LeaseClaim> {
        async move {
            let mut backend = RedisLeases::new(self.conn.clone());
            match acquire_run_lease(&mut backend, run_id, step_ms.max(1) as u64).await {
                RunClaim::Claimed(token) => LeaseClaim::Claimed { token: token.value },
                RunClaim::Busy => LeaseClaim::Busy,
                RunClaim::Blocked(error) => LeaseClaim::Blocked { error },
            }
        }
        .boxed()
    }

    fn renew<'a>(
        &'a self,
        run_id: &'a str,
        token: &'a str,
        step_ms: i64,
    ) -> BoxFuture<'a, LeaseRenewal> {
        async move {
            let mut backend = RedisLeases::new(self.conn.clone());
            let tok = LeaseToken {
                key: run_lease_key(run_id),
                value: token.to_string(),
            };
            match renew_lease(&mut backend, &tok, step_ms.max(1) as u64).await {
                LeaseResult::Ok => LeaseRenewal::Ok,
                LeaseResult::Lost => LeaseRenewal::Lost,
                LeaseResult::Unavailable(_) => LeaseRenewal::Unavailable,
            }
        }
        .boxed()
    }

    fn release<'a>(&'a self, run_id: &'a str, token: &'a str) -> BoxFuture<'a, ()> {
        async move {
            // Failure here is survivable and must not mask the run's own
            // outcome: the key carries a TTL, so the worst case is that the
            // next driver waits it out.
            let mut backend = RedisLeases::new(self.conn.clone());
            let tok = LeaseToken {
                key: run_lease_key(run_id),
                value: token.to_string(),
            };
            if let LeaseResult::Unavailable(e) = release_run_lease(&mut backend, &tok).await {
                tracing::error!("{LOG} could not release the lease for {run_id}: {e}");
            }
        }
        .boxed()
    }
}

// ── What a device sees ───────────────────────────────────────────────────────

/// The event published on every persisted transition. Small on purpose: it
/// carries enough to render a list row and to decide whether to refetch, and
/// nothing that could disagree with the table.
#[derive(Debug, Clone, Serialize)]
pub struct RunEvent {
    #[serde(rename = "type")]
    pub kind_tag: &'static str, // always "run"
    pub run_id: String,
    pub kind: String,
    pub state: RunState,
    pub phase: String,
    /// Present only on the transition into `awaiting`, so a device can raise
    /// the question without a round trip. THE QUESTION DOES NOT GO ON THE
    /// WIRE: who may READ a decision's text is the definition's `audience`
    /// while who may WATCH a run is its subject's read ACL — different sets,
    /// overlapping in the common case and not guaranteed to. The real
    /// publisher (realtime) names the fields one at a
    /// time on the way out so `question` cannot ride along by accident.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<DecisionRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RunEvent {
    fn transition(run_id: &str, kind: &str, state: RunState, phase: &str) -> Self {
        Self {
            kind_tag: "run",
            run_id: run_id.to_string(),
            kind: kind.to_string(),
            state,
            phase: phase.to_string(),
            question: None,
            error: None,
        }
    }
}

pub type PublishFn = Arc<dyn Fn(RunEvent, Option<&str>) + Send + Sync>;

/// What `pause` (decide.rs, next file) answers the driver. The ok half carries
/// how many people were actually told — ZERO IS A REAL ANSWER and it is not a
/// failure of the pause: the row is `awaiting` and durable either way, with an
/// UNMARKED key the approvals sweep announces on its next tick.
#[derive(Debug, Clone)]
pub enum PauseOutcome {
    Parked {
        approval_key: String,
        announced: usize,
    },
    /// The park did not land, and every reason is a normal one: another
    /// instance owns the run now, somebody cancelled it, the row is gone. The
    /// question is simply not asked; nothing is half-parked.
    Refused {
        reason: WriteFailure,
        state: Option<RunState>,
    },
}

pub type PauseFn = Arc<dyn Fn(PauseArgs) -> BoxFuture<'static, PauseOutcome> + Send + Sync>;

pub struct PauseArgs {
    pub run_id: String,
    pub token: String,
    pub question: DecisionRequest,
    pub phase: Option<String>,
}

// ── Deps ─────────────────────────────────────────────────────────────────────

pub type DefinitionForFn = Arc<dyn Fn(&str) -> Option<Arc<RunDefinition>> + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;
pub type NewIdFn = Arc<dyn Fn() -> String + Send + Sync>;

/// Every edge to the outside world, overridable per call. Cloned into the
/// detached drive and the renewal task; all fields are
/// Arc so the clone is four pointer bumps.
#[derive(Clone)]
pub struct RunDeps {
    pub store: Arc<dyn RunStore>,
    pub lease: Arc<dyn RunLease>,
    pub publish: PublishFn,
    /// THE `running → awaiting` transition — park the run on the question,
    /// file it as an approval, tell whoever the definition's `audience` names.
    /// The driver does not write it itself: `pause` and `decide` are the two
    /// ends of one transition, and rules about who may be told what a
    /// question says belong next to the rules about who may answer it.
    pub pause: PauseFn,
    pub definition_for: DefinitionForFn,
    pub now: NowFn,
    pub new_id: NewIdFn,
}

impl RunDeps {
    /// The default lookup is the global registry — kinds register at module
    /// load — with every other edge still the caller's to supply. The full
    /// real assembly (PgRunStore, RedisRunLease, the realtime publish, decide's
    /// pause) is `real_run_deps` in runs/mod.rs.
    #[allow(clippy::too_many_arguments)]
    pub fn with_registry_lookup(
        store: Arc<dyn RunStore>,
        lease: Arc<dyn RunLease>,
        publish: PublishFn,
        pause: PauseFn,
        now: NowFn,
        new_id: NewIdFn,
    ) -> Self {
        Self {
            store,
            lease,
            publish,
            pause,
            definition_for: Arc::new(registry_definition),
            now,
            new_id,
        }
    }
}

// ── Enqueue ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct EnqueueOptions {
    /// Whose run it is. None for org-wide work with nobody behind it.
    pub owner_user_id: Option<String>,
    /// What it is about — 'task' / 'channel' / 'conversation' / 'research'.
    pub subject_type: Option<String>,
    pub subject_id: Option<String>,
    /// The first line a waiting human reads, before the first `ctx.log`.
    pub phase: Option<String>,
    /// Supply the id, for a caller that must reference the run in the same
    /// transaction it creates it from.
    pub id: Option<String>,
    /// Begin driving immediately, detached from this request. Default true.
    ///
    /// A NICETY, NOT THE GUARANTEE. The detached drive is what makes a run
    /// start in the same second the button was pressed; the RECLAIM SWEEP is
    /// what makes it finish. If this process dies between the insert and the
    /// first checkpoint, the row is `queued` with no lease and the sweep takes
    /// it — no part of the durability story depends on this promise being
    /// awaited.
    pub start: Option<bool>,
}

/// Write the row, publish it, return. Never waits for the work.
///
/// Returns the row rather than an id because the caller almost always wants to
/// render it immediately, and a caller that has to re-read what it just wrote
/// is a caller that will render a state the database has not got round to.
pub async fn enqueue(
    def: &Arc<RunDefinition>,
    input: Value,
    opts: EnqueueOptions,
    deps: &RunDeps,
) -> Result<RunRow, sqlx::Error> {
    // A run whose kind nothing has registered can be started by THIS process
    // and then never resumed by any other — the sweep would find the row and
    // have no code to advance it. That is a wiring bug in this build, so say
    // it at enqueue time when the stack still names the caller.
    if (deps.definition_for)(&def.kind).is_none() {
        tracing::error!(
            "{LOG} enqueuing kind \"{}\", which is not registered — it cannot be reclaimed after \
             a restart. Call register_run at module load.",
            def.kind
        );
    }

    let run = deps
        .store
        .insert(NewRun {
            id: opts.id.clone().unwrap_or_else(|| (deps.new_id)()),
            kind: def.kind.clone(),
            owner_user_id: opts.owner_user_id,
            subject_type: opts.subject_type,
            subject_id: opts.subject_id,
            input,
            phase: opts.phase.unwrap_or_else(|| "queued".into()),
        })
        .await?;

    // THE ORDERING RULE, first instance: the row exists before anybody is told
    // it does. A publish that outran the write is how a second device renders a
    // run the database does not have.
    (deps.publish)(
        RunEvent::transition(&run.id, &run.kind, run.state, &run.phase),
        run.owner_user_id.as_deref(),
    );

    if opts.start != Some(false) {
        let deps = deps.clone();
        let run_id = run.id.clone();
        tokio::spawn(async move {
            if let Err(e) = drive(&run_id, &deps).await {
                tracing::error!("{LOG} detached drive of {run_id} threw: {e}");
            }
        });
    }
    Ok(run)
}

// ── Drive ────────────────────────────────────────────────────────────────────

/// Why this drive stopped. Most of these are perfectly healthy, and the point
/// of spelling them out is that a caller (and a log line) can tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveStop {
    Done,
    Error,
    /// Parked on a human decision.
    Awaiting,
    /// A `retry` result: scheduled, nobody bothered, no attempt consumed.
    Deferred,
    Cancelled,
    /// Another instance owns it now. Clean.
    LeaseLost,
    /// Somebody else is driving it, or a deferral has not elapsed. Clean.
    Busy,
    /// Redis could not be asked whether this driver would be alone, so the row
    /// was left ALONE — not driven, and above all not failed.
    Blocked,
    Missing,
    /// Nothing in THIS process knows this kind. Not a failure of the run.
    NoDefinition,
    /// Reclaimed more times than the definition allows; filed as an error.
    Exhausted,
    /// The row is in a state no driver advances (`awaiting`, or terminal).
    NotRunnable,
}

#[derive(Debug, Clone)]
pub struct DriveResult {
    pub run_id: String,
    pub stop: DriveStop,
    /// How many times `step()` was entered on THIS drive.
    pub steps: u32,
    pub state: Option<RunState>,
    pub error: Option<String>,
    /// For `deferred`: how long until it may be taken again.
    pub retry_after_ms: Option<u64>,
}

impl DriveResult {
    fn stop(run_id: &str, stop: DriveStop, steps: u32, state: Option<RunState>) -> Self {
        Self {
            run_id: run_id.to_string(),
            stop,
            steps,
            state,
            error: None,
            retry_after_ms: None,
        }
    }
}

/// Why an in-flight step was cut loose. Not exported past this module:
/// nothing outside the loop should be able to fake one.
#[derive(Debug, Clone, Copy)]
enum StepInterrupt {
    Deadline,
    /// The renewal loop lost the lease while the step was in flight.
    LeaseLost,
}

/// Clamp phase text at the given number of BYTES, on a char boundary — the
/// cut lands before the boundary, never inside a character.
pub(crate) fn clamp_text(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The first line of an error text — a stack dump in a run row is a row
/// nobody reads.
fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").to_string()
}

/// Take the run and advance it until it finishes, pauses, or stops being ours.
///
/// Safe to call from anywhere, on any instance, as often as you like: a run
/// somebody else is driving comes back `busy` after one Redis round trip.
pub async fn drive(run_id: &str, deps: &RunDeps) -> Result<DriveResult, sqlx::Error> {
    let existing = match deps.store.get(run_id).await? {
        Some(row) => row,
        None => return Ok(DriveResult::stop(run_id, DriveStop::Missing, 0, None)),
    };
    let def = match (deps.definition_for)(&existing.kind) {
        Some(def) => def,
        None => {
            // NOT an error on the row. A kind this build cannot define — a
            // row from a newer deploy, or a def outside this build — is still
            // perfectly drivable by an instance that can, and failing the run
            // here would destroy work on the strength of a local registry.
            tracing::warn!(
                "{LOG} {run_id}: no definition registered for kind \"{}\" on this instance — \
                 leaving it for one that has it",
                existing.kind
            );
            return Ok(DriveResult::stop(
                run_id,
                DriveStop::NoDefinition,
                0,
                Some(existing.state),
            ));
        }
    };
    // `is_drivable`, not a state comparison spelled out again here. There is
    // one rule about which states a driver may pick up and `awaiting` is the
    // reason it is worth centralizing: a run parked on a person is healthy,
    // not stuck. reclaim calls the same function.
    if !is_drivable(existing.state) {
        return Ok(DriveResult::stop(
            run_id,
            DriveStop::NotRunnable,
            0,
            Some(existing.state),
        ));
    }

    // The lease TTL is the step budget: a driver that dies mid-step is
    // reclaimable one step-length after it stops renewing, which is the
    // shortest safe answer available without guessing. The floor keeps a
    // definition with a very small `max_step_ms` from making its own lease
    // unrenewable.
    let lease_ms: i64 = 5_000.max(def.max_step_ms as i64);

    let token = match deps.lease.acquire(run_id, lease_ms).await {
        LeaseClaim::Claimed { token } => token,
        LeaseClaim::Busy => {
            return Ok(DriveResult::stop(
                run_id,
                DriveStop::Busy,
                0,
                Some(existing.state),
            ));
        }
        LeaseClaim::Blocked { error } => {
            // FAIL CLOSED, exactly as the scheduler does. An unreachable Redis
            // is not permission to run unguarded; two instances advancing one
            // run is how the side effect happens twice. The row is untouched —
            // not failed, not rewritten — so a later sweep takes it from the
            // same checkpoint.
            tracing::error!("{LOG} {run_id}: lease unavailable, not driving: {error}");
            return Ok(DriveResult {
                error: Some(first_line(&error)),
                ..DriveResult::stop(run_id, DriveStop::Blocked, 0, Some(existing.state))
            });
        }
    };

    let (mut row, reclaimed) = match deps.store.claim(run_id, &token, lease_ms).await? {
        ClaimOutcome::Claimed { run, reclaimed } => (*run, reclaimed),
        // The row disagreed with the lease. Give the lease straight back
        // rather than letting it time out, or a run that was merely raced
        // would be unclaimable for a whole TTL.
        ClaimOutcome::Missing => {
            deps.lease.release(run_id, &token).await;
            return Ok(DriveResult::stop(run_id, DriveStop::Missing, 0, None));
        }
        ClaimOutcome::Taken { state, .. } => {
            deps.lease.release(run_id, &token).await;
            return Ok(DriveResult::stop(run_id, DriveStop::Busy, 0, Some(state)));
        }
        ClaimOutcome::NotRunnable { state } => {
            deps.lease.release(run_id, &token).await;
            return Ok(DriveResult::stop(
                run_id,
                DriveStop::NotRunnable,
                0,
                Some(state),
            ));
        }
    };
    let max_attempts = def.max_attempts.max(1);

    // `attempt` counts ENTRIES that followed a crash, so this is "how many
    // drivers has this run killed". A run over the line is filed as an error
    // with the count in the message: it is a bug report, and a bug report that
    // reads 'failed' with no number is one nobody can act on.
    if row.attempt >= max_attempts as i32 {
        let message = format!(
            "run gave up after {} attempt(s): each driver that took it stopped without finishing \
             or checkpointing",
            row.attempt
        );
        tracing::error!("{LOG} {run_id} ({}): {message}", row.kind);
        let write = deps.store.fail(run_id, &token, message.clone()).await?;
        deps.lease.release(run_id, &token).await;
        if write.is_ok() {
            let mut ev = RunEvent::transition(run_id, &row.kind, RunState::Error, &row.phase);
            ev.error = Some(message.clone());
            (deps.publish)(ev, row.owner_user_id.as_deref());
        }
        return Ok(DriveResult {
            error: Some(message),
            ..DriveResult::stop(run_id, DriveStop::Exhausted, 0, Some(RunState::Error))
        });
    }

    if reclaimed {
        tracing::warn!(
            "{LOG} {run_id} ({}) reclaimed at attempt {} of {max_attempts}, phase \"{}\" — the \
             previous driver stopped without releasing. Its last step re-runs from the persisted \
             checkpoint.",
            row.kind,
            row.attempt,
            row.phase
        );
    }

    // ── Lease renewal ──────────────────────────────────────────────────────────
    // TWO WRITES PER BEAT, and both matter: Redis is what another instance
    // TESTS before it takes the run, and the row's `lease_expires_at` is what
    // the reclaim query SCANS (Redis has no index over "every run whose lease
    // expired"). That pairing is why the driver runs its own renewal loop
    // instead of the lease module's heartbeat — that heartbeat pumps Redis,
    // correctly, and knows nothing about the row that has to agree with it.
    //
    // Any beat that does not come back ok aborts the step and stops the drive.
    // A renewal that cannot REACH Redis has not necessarily lost the lease,
    // but it can no longer prove it holds one, and continuing to write on a
    // lease we cannot verify is the one thing that turns a blip into two
    // drivers.
    let lost = Arc::new(AtomicBool::new(false));
    let (abort_tx, _abort_rx) = watch::channel(false);
    let renew_every = Duration::from_millis(1_000.max((lease_ms / 3) as u64));
    let renew_task = {
        let deps = deps.clone();
        let run_id = run_id.to_string();
        let token = token.clone();
        let lost = lost.clone();
        let abort_tx = abort_tx.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(renew_every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await; // the interval fires immediately once by contract
            loop {
                tick.tick().await;
                match deps.lease.renew(&run_id, &token, lease_ms).await {
                    LeaseRenewal::Ok => {
                        if let Err(reason) = deps.store.heartbeat(&run_id, &token, lease_ms).await {
                            // The Redis lease is still ours but the row will
                            // not confirm it. The honest reading: stop
                            // persisting, let
                            // a sweep decide from the checkpoint.
                            tracing::error!(
                                "{LOG} {run_id}: heartbeat refused ({reason:?}) — stopping the \
                                 drive"
                            );
                            lost.store(true, Ordering::SeqCst);
                            abort_tx.send(true).ok();
                            break;
                        }
                    }
                    LeaseRenewal::Unavailable => {
                        tracing::error!(
                            "{LOG} {run_id}: could not renew the lease (Redis unreachable) — \
                             stopping; a later sweep resumes the run"
                        );
                        lost.store(true, Ordering::SeqCst);
                        abort_tx.send(true).ok();
                        break;
                    }
                    LeaseRenewal::Lost => {
                        tracing::warn!(
                            "{LOG} {run_id}: lost the lease mid-run — another instance owns it \
                             now, stopping cleanly"
                        );
                        lost.store(true, Ordering::SeqCst);
                        abort_tx.send(true).ok();
                        break;
                    }
                }
            }
        })
    };

    // ── Progress lines ───────────────────────────────────────────────────────
    // `ctx.log` is fire-and-forget for the step, but it is NOT unordered: each
    // call is queued in order, and the loop FLUSHES the queue at every
    // boundary — persist, then publish, one line at a time. A phase line that
    // outran its write is a device showing a sentence the database will never
    // confirm. (The queue-and-flush keeps that order, with the writes starting
    // at the boundary instead of mid-step.)
    let progress = Arc::new(ProgressLog {
        queue: Mutex::new(Vec::new()),
        phase: Mutex::new(row.phase.clone()),
    });
    let log: Arc<dyn Fn(String) + Send + Sync> = {
        let progress = progress.clone();
        Arc::new(move |next: String| {
            let text = clamp_text(&next, 300);
            *progress.phase.lock().unwrap() = text.clone();
            progress.queue.lock().unwrap().push(text);
        })
    };

    let mut steps: u32 = 0;
    let loop_result = drive_loop(
        run_id,
        &token,
        def.clone(),
        deps,
        &lost,
        &abort_tx,
        &progress,
        &log,
        &mut row,
        &mut steps,
    )
    .await;

    // ── Cleanup ─────────────────────────────────────────────────────────────
    // Runs on EVERY exit path, including a database error thrown out of the
    // loop: a drive that died holding the lease is the one drive that must not
    // keep it.
    renew_task.abort();
    let retry_after_ms = loop_result.as_ref().ok().and_then(|r| r.retry_after_ms);
    if let Some(after) = retry_after_ms {
        // HOLD the lease for the wait. Releasing it would let the next sweep
        // take the run immediately, turning "come back in thirty seconds" into
        // a hot loop against whatever asked for the wait in the first place.
        let held = deps.lease.renew(run_id, &token, 1.max(after as i64)).await;
        if !matches!(held, LeaseRenewal::Ok) {
            tracing::warn!(
                "{LOG} {run_id}: could not hold the deferral in Redis ({held:?}) — the row's own \
                 expiry still gates it"
            );
        }
    } else {
        // Give it back rather than letting it time out: a run this driver
        // finished, parked or handed over should be takeable now, not one TTL
        // from now. `release` is compare-and-delete, so a lease that already
        // moved on is left alone.
        deps.lease.release(run_id, &token).await;
        // And drop the row's lease stamp, but ONLY if it is still ours and
        // still running — every terminal write above already cleared it, so
        // this is the clean-stop path (cancelled, lease-lost, a row that
        // moved under us).
        if let Err(e) = deps.store.release(run_id, &token).await {
            tracing::error!("{LOG} {run_id}: could not clear the row lease: {e}");
        }
    }
    loop_result
}

struct ProgressLog {
    queue: Mutex<Vec<String>>,
    phase: Mutex<String>,
}

/// Flush the pending phase lines in order, persist-then-publish each. A line
/// whose write is refused is dropped quietly: the boundary check reports why,
/// and a log line does not get to shout about it.
async fn flush_progress(
    progress: &ProgressLog,
    run_id: &str,
    token: &str,
    kind: &str,
    owner: Option<&str>,
    deps: &RunDeps,
) -> Result<(), sqlx::Error> {
    let pending = std::mem::take(&mut *progress.queue.lock().unwrap());
    for text in pending {
        if deps.store.phase(run_id, token, text.clone()).await?.is_ok() {
            (deps.publish)(
                RunEvent::transition(run_id, kind, RunState::Running, &text),
                owner,
            );
        }
    }
    Ok(())
}

/// The step loop, extracted so `drive` can run its cleanup on every exit path
/// without a triply-nested closure.
#[allow(clippy::too_many_arguments)]
async fn drive_loop(
    run_id: &str,
    token: &str,
    def: Arc<RunDefinition>,
    deps: &RunDeps,
    lost: &Arc<AtomicBool>,
    abort_tx: &watch::Sender<bool>,
    progress: &Arc<ProgressLog>,
    log: &Arc<dyn Fn(String) + Send + Sync>,
    row: &mut RunRow,
    steps: &mut u32,
) -> Result<DriveResult, sqlx::Error> {
    let phase = || progress.phase.lock().unwrap().clone();
    let owner = row.owner_user_id.clone();
    let owner = owner.as_deref();

    loop {
        // ── The step boundary ────────────────────────────────────────────────
        // Everything that can stop this drive is checked HERE, before any work
        // is done, and the row is re-read from the store rather than carried
        // in memory. That read is worth its round trip twice over: it is the
        // cancellation check (any instance, honored here), and it means the
        // checkpoint handed to the next step is the PERSISTED one — resume is
        // the same code path as ordinary progress, not a second one that could
        // drift.
        flush_progress(progress, run_id, token, &def.kind, owner, deps).await?;
        if lost.load(Ordering::SeqCst) {
            return Ok(DriveResult::stop(
                run_id,
                DriveStop::LeaseLost,
                *steps,
                Some(row.state),
            ));
        }

        let fresh = match deps.store.get(run_id).await? {
            Some(fresh) => fresh,
            None => {
                return Ok(DriveResult::stop(run_id, DriveStop::Missing, *steps, None));
            }
        };
        *row = fresh;
        if row.state == RunState::Cancelled {
            tracing::info!(
                "{LOG} {run_id} ({}): cancelled, stopping at step {}",
                row.kind,
                steps
            );
            return Ok(DriveResult::stop(
                run_id,
                DriveStop::Cancelled,
                *steps,
                Some(row.state),
            ));
        }
        if row.lease_owner.as_deref() != Some(token) {
            return Ok(DriveResult::stop(
                run_id,
                DriveStop::LeaseLost,
                *steps,
                Some(row.state),
            ));
        }
        if row.state != RunState::Running {
            return Ok(DriveResult::stop(
                run_id,
                DriveStop::NotRunnable,
                *steps,
                Some(row.state),
            ));
        }

        // ── One step ─────────────────────────────────────────────────────────
        let answer: Option<DecisionAnswer> = row.decision.as_ref().and_then(|d| d.answer.clone());
        let ctx = RunStepContext {
            run: row.clone(),
            input: row.input.clone(),
            checkpoint: row.checkpoint.clone(),
            decision: answer.clone(),
            signal: super::define::StepSignal::from_sender(abort_tx),
            log: log.clone(),
            attempt: row.attempt,
        };

        *steps += 1;
        let raced = tokio::select! {
            res = (def.step)(ctx) => match res {
                Ok(result) => StepRace::Finished(result),
                Err(message) => StepRace::Erred(message),
            },
            // The deadline is the definition's own statement of how long one
            // unit of progress takes — not a guess about it.
            _ = tokio::time::sleep(Duration::from_millis(def.max_step_ms)) => {
                StepRace::Interrupted(StepInterrupt::Deadline)
            }
            // A lease lost mid-step reaches the step as `ctx.signal`, but a
            // step is ALLOWED to ignore its signal, and the driver must still
            // stop: dropping the future cancels it at its next await.
            _ = abort_fires(abort_tx) => StepRace::Interrupted(StepInterrupt::LeaseLost),
        };
        if matches!(raced, StepRace::Interrupted(StepInterrupt::Deadline)) {
            // Tell any detached work the step spawned, best effort.
            abort_tx.send(true).ok();
        }

        let result = match raced {
            StepRace::Finished(result) => result,
            StepRace::Erred(message) => {
                // A step that ERRED. Filed as an error row with the
                // first line of the message on it.
                tracing::error!(
                    "{LOG} {run_id} ({}) step threw at phase \"{}\": {message}",
                    row.kind,
                    phase()
                );
                flush_progress(progress, run_id, token, &def.kind, owner, deps).await?;
                if let Err(reason) = deps.store.fail(run_id, token, first_line(&message)).await? {
                    return Ok(stop_from_write(run_id, *steps, reason, row.state));
                }
                let mut ev = RunEvent::transition(run_id, &row.kind, RunState::Error, &phase());
                ev.error = Some(first_line(&message));
                (deps.publish)(ev, owner);
                return Ok(DriveResult {
                    error: Some(first_line(&message)),
                    ..DriveResult::stop(run_id, DriveStop::Error, *steps, Some(RunState::Error))
                });
            }
            StepRace::Interrupted(StepInterrupt::LeaseLost) => {
                return Ok(DriveResult::stop(
                    run_id,
                    DriveStop::LeaseLost,
                    *steps,
                    Some(row.state),
                ));
            }
            StepRace::Interrupted(StepInterrupt::Deadline) => {
                // A step that blew its own declared budget. It is filed as an
                // error and NOT retried: re-entering the run puts the
                // machinery in the position of authorizing a second copy of a
                // step whose first copy may (through detached work) still be
                // in flight. `max_step_ms` is the definition's OWN statement
                // of how long a unit of progress takes; blowing it is a bug,
                // and a visible error row is how a bug gets fixed.
                let message = format!(
                    "step exceeded maxStepMs ({}ms) at phase \"{}\". The step may still be \
                     running; it will not be retried.",
                    def.max_step_ms,
                    phase()
                );
                tracing::error!("{LOG} {run_id} ({}): {message}", row.kind);
                flush_progress(progress, run_id, token, &def.kind, owner, deps).await?;
                if let Err(reason) = deps.store.fail(run_id, token, message.clone()).await? {
                    return Ok(stop_from_write(run_id, *steps, reason, row.state));
                }
                let mut ev = RunEvent::transition(run_id, &row.kind, RunState::Error, &phase());
                ev.error = Some(message.clone());
                (deps.publish)(ev, owner);
                return Ok(DriveResult {
                    error: Some(message),
                    ..DriveResult::stop(run_id, DriveStop::Error, *steps, Some(RunState::Error))
                });
            }
        };
        flush_progress(progress, run_id, token, &def.kind, owner, deps).await?;

        // ── Apply the result ─────────────────────────────────────────────────
        //
        // THE ORDERING RULE, everywhere below: PERSIST, THEN PUBLISH. A
        // publish that outran its write is how a second device renders state
        // the database does not have. And where the ordering allows it,
        // persist BEFORE the side effect: the checkpoint write below happens
        // before the next step (which is where the side effects live), so a
        // crash costs at most the one step that had not checkpointed yet.
        match result {
            StepResult::Next {
                checkpoint,
                phase: next_phase,
            } => {
                if let Some(p) = next_phase {
                    *progress.phase.lock().unwrap() = clamp_text(&p, 300);
                }
                let phase_now = phase();
                let write = deps
                    .store
                    .checkpoint(
                        run_id,
                        token,
                        checkpoint,
                        phase_now.clone(),
                        // Clear the answer the step just consumed, in the SAME
                        // write that records the progress it produced. Two
                        // writes would leave a window where a reclaim hands
                        // the next step an answer to a question that has
                        // already been acted on, and it would act on it again.
                        answer.is_some(),
                    )
                    .await?;
                if let Err(reason) = write {
                    return Ok(stop_from_write(run_id, *steps, reason, row.state));
                }
                (deps.publish)(
                    RunEvent::transition(run_id, &def.kind, RunState::Running, &phase_now),
                    owner,
                );
                continue;
            }
            StepResult::Done { result: value } => {
                if let Err(reason) = deps.store.complete(run_id, token, value).await? {
                    return Ok(stop_from_write(run_id, *steps, reason, row.state));
                }
                (deps.publish)(
                    RunEvent::transition(run_id, &def.kind, RunState::Done, &phase()),
                    owner,
                );
                return Ok(DriveResult::stop(
                    run_id,
                    DriveStop::Done,
                    *steps,
                    Some(RunState::Done),
                ));
            }
            StepResult::Decide { question } => {
                // ONE PARK, and it is not here. `pause` derives the approval
                // key, does the compare-and-set park, publishes and announces.
                // It cannot fail the run: every refusal it returns is a normal
                // outcome and maps onto the same stops as any other refused
                // write, while a delivery that did not happen leaves the row
                // `awaiting` with an UNMARKED key for the approvals sweep.
                let parked = (deps.pause)(PauseArgs {
                    run_id: run_id.to_string(),
                    token: token.to_string(),
                    question: question.clone(),
                    phase: Some(phase()),
                })
                .await;
                match parked {
                    PauseOutcome::Parked { .. } => {
                        return Ok(DriveResult::stop(
                            run_id,
                            DriveStop::Awaiting,
                            *steps,
                            Some(RunState::Awaiting),
                        ));
                    }
                    PauseOutcome::Refused { reason, state } => {
                        return Ok(stop_from_write(
                            run_id,
                            *steps,
                            reason,
                            state.unwrap_or(row.state),
                        ));
                    }
                }
            }
            StepResult::Retry { after, reason } => {
                // A SOFT pause. No notification, no attempt consumed, state
                // back to `queued` — the wait is expressed as a lease that has
                // not expired yet, which is the same thing every other "not
                // yet" in this file means.
                let after = after.as_millis() as u64;
                let until = (deps.now)() + after as i64;
                let reason = clamp_text(&reason, 300);
                if let Err(f) = deps
                    .store
                    .defer(run_id, token, until, reason.clone())
                    .await?
                {
                    return Ok(stop_from_write(run_id, *steps, f, row.state));
                }
                (deps.publish)(
                    RunEvent::transition(run_id, &def.kind, RunState::Queued, &reason),
                    owner,
                );
                return Ok(DriveResult {
                    retry_after_ms: Some(after),
                    ..DriveResult::stop(run_id, DriveStop::Deferred, *steps, Some(RunState::Queued))
                });
            }
        }
    }
}

/// Resolves when the drive's abort flips true (immediately if it already was).
/// Subscribe-first so a flip between the check and the await is still seen.
async fn abort_fires(tx: &watch::Sender<bool>) {
    let mut rx = tx.subscribe();
    if *rx.borrow() {
        return;
    }
    let _ = rx.changed().await;
}

enum StepRace {
    Finished(StepResult),
    Erred(String),
    Interrupted(StepInterrupt),
}

/// Map a refused compare-and-set onto a stop. Every branch is a NORMAL
/// outcome — somebody cancelled it, somebody else took it, the row went away —
/// and none of them is an error the run should be marked with.
fn stop_from_write(run_id: &str, steps: u32, reason: WriteFailure, state: RunState) -> DriveResult {
    match reason {
        WriteFailure::Cancelled => {
            tracing::info!(
                "{LOG} {run_id}: cancelled while a step was running — the step's result is \
                 discarded"
            );
            DriveResult::stop(
                run_id,
                DriveStop::Cancelled,
                steps,
                Some(RunState::Cancelled),
            )
        }
        WriteFailure::Missing => DriveResult::stop(run_id, DriveStop::Missing, steps, None),
        WriteFailure::LeaseLost { .. } => {
            tracing::info!("{LOG} {run_id}: another instance owns this run now, stopping cleanly");
            DriveResult::stop(run_id, DriveStop::LeaseLost, steps, Some(state))
        }
        WriteFailure::State { .. } => {
            DriveResult::stop(run_id, DriveStop::NotRunnable, steps, Some(state))
        }
    }
}

// ── The sweep, and the answer, both deliberately absent ──────────────────────
//
// THE RECLAIM SWEEP IS runs/reclaim.rs. Two sweepers over one table is not a
// redundancy — it is two different answers to "may this row be woken", which
// is the one question in this system that must have exactly one. reclaim is
// the registered job, it re-states the `awaiting` guard in code rather than
// trusting an index definition, and it reports what a pass did.
//
// THE ANSWER TO A PARKED QUESTION IS runs/decide.rs. An exported, ungated
// `store.answer + publish + drive` beside the gated one would make the gate a
// convention: any route could import the wrong name and resume a run on behalf
// of somebody entitled to nothing. The ungated write stays private inside
// decide, behind `decide()`, which resolves the definition's authority and
// asks `may_decide_content` before it writes.

/// Stop a run, from anywhere. The driver that owns it finds out at its next
/// step boundary — or when its next write is refused, whichever comes first.
pub async fn cancel_run(
    run_id: &str,
    reason: Option<String>,
    deps: &RunDeps,
) -> Result<CancelOutcome, sqlx::Error> {
    let res = deps.store.cancel(run_id, reason).await?;
    if matches!(res, CancelOutcome::Cancelled { .. })
        && let Some(run) = deps.store.get(run_id).await?
    {
        (deps.publish)(
            RunEvent::transition(&run.id, &run.kind, run.state, &run.phase),
            run.owner_user_id.as_deref(),
        );
    }
    Ok(res)
}

/// What this person has in flight, for the strip and the list.
pub async fn active_runs(user_id: &str, deps: &RunDeps) -> Result<Vec<RunRow>, sqlx::Error> {
    deps.store.active_for(user_id, None).await
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_text_clamps_at_a_char_boundary() {
        // 300 bytes of ASCII plus a multibyte tail: the clamp lands before the
        // boundary, never inside a character.
        let text = "a".repeat(299) + "é你";
        let clamped = clamp_text(&text, 300);
        assert_eq!(clamped, "a".repeat(299));
        assert_eq!(clamp_text("short", 300), "short");
    }

    #[test]
    fn first_line_keeps_the_message_and_drops_the_trace() {
        assert_eq!(first_line("boom\nat foo.rs:1"), "boom");
        assert_eq!(first_line("only"), "only");
    }
}
