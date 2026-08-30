// The background scheduler — the one place periodic server work is timed.
// Port of ui/src/server/scheduler.ts, whole: the timing, the guarantees and
// the Redis-lease POLICY (the mechanism is runs/lease.rs, already crossed;
// the demote-on-completion hold below is the scheduler's own idea, and the
// comment there says so).
//
// WHAT THIS FILE GUARANTEES
//   1. It runs with zero traffic. `start_scheduler` is called once from the
//      server's boot path; the timers are the only trigger. Nothing in a
//      request path decides when background work happens.
//   2. A failing job does not kill the process, does not stop its own
//      schedule, and does NOT fail silently. Every failure is logged with the
//      job name, the elapsed time and the error, and is kept in
//      `scheduler_status` for the health walk. A job that PANICS is contained
//      the same way — the attempt runs inside `catch_unwind`, because a panic
//      that skipped the cleanup path would leave `running` stuck true and the
//      overlap guard would turn away every tick behind it forever: the M1
//      cold-boot wedge, rebuilt in Rust by accident.
//   3. A job never overlaps itself — not in this process (the `running` flag)
//      and not across processes (the Redis lease).
//   4. Two instances do not double-run a job. The lease is held for the rest
//      of the interval after a run COMPLETES (demote, not release): a mutex
//      only stops two instances running the job at the same moment, and would
//      still let instance B run comms decay a minute after instance A
//      finished — twice per interval, which for these jobs is user-visible
//      harm (comms decay archives conversations; the outreach sweep sends
//      messages). One run per interval, fleet-wide.
//   5. Nothing here dangles a task on shutdown. `stop_scheduler` aborts the
//      timers immediately (a redeploy stops STARTING runs the moment it is
//      told to go) and waits, briefly, for whatever is in flight.
//   6. If Redis is unreachable the tick is SKIPPED, not run unguarded. Fail
//      closed: a missed hour of archiving is recoverable, a duplicate
//      proactive DM to a customer is not. The skip is logged, and the next
//      tick retries.
//
// THE ONE EXCEPTION is `per_instance` (see `JobSpec`): a job whose entire
// input is an in-memory queue in THIS process has nothing shared to
// duplicate, and leasing it would leave every instance but the lease-winner's
// queue undrained. It keeps the in-process overlap guard either way.
//
// SEEING FAILURES
//   `scheduler_status` is not decoration — guarantee 2 is only worth
//   anything if someone reads it, so `unhealthy_jobs` distils the same state
//   into the sentences an operator needs. Five things count as unhealthy,
//   and the two in the middle are the M1 cold-boot lesson written down: a
//   job that neither fails nor returns has `failures == 0` and `running`
//   stuck true, and a counter-only check cannot see it from either side. A
//   hang is the failure mode a scheduler has to name out loud, because it is
//   the only one that produces no error to log.
//
// THE ARMING SWITCH
//   `TALARIA_SCHEDULER` is one variable both runtimes read, with three
//   postures. Unset (or anything unrecognized) is today's world: TS arms,
//   Rust arms nothing. `off` is the kill switch on either runtime — the one
//   to deploy behind if a job ever misbehaves. `rust` is THE FLIP: this
//   process registers and arms the whole table (jobs::arm, from main) while
//   TS's startScheduler stands down, so the handoff is one value in one env
//   file, never a window where both runtimes think a period is theirs. The
//   'sched' lease namespace is shared besides, so even a botched flip is a
//   contest between holders of the same keys, not a double-fire: the loser
//   of a lease attempt skips that interval.

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, join_all};
use tokio::task::JoinHandle;

use crate::agent_auth::{epoch_ms_to_iso, iso_to_epoch_ms};
use crate::runs::lease::{
    self, AcquireResult, HeartbeatOptions, LeaseBackend, LeaseHeartbeat, LeaseHolder, LeaseToken,
    RedisLeases, instance_id, keep_lease_alive, lease_key,
};

const LOG: &str = "[scheduler]";
const DEFAULT_MAX_RUN_MS: u64 = 10 * 60_000;
/// The lease namespace TS and Rust share, so the flip is a handoff between
/// holders of the same keys.
const SCHED_LEASE_NS: &str = "sched";

/// Every job this deployment expects to be running. Adding a variant here
/// without registering it fails the boot check in `start_scheduler`, which is
/// the point: a job whose module never got imported would otherwise be
/// invisible — the exact failure mode (background work that silently does not
/// happen) this file exists to end. `BlurbRewrite` is the one this batch ADDS:
/// its only TS trigger was the `/api/models` handler, which the coexistence
/// proxy shadows — in any proxied environment the org-voice sweep is dark
/// until this job carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JobName {
    CommsDecay,
    OutreachSweep,
    PriceRefresh,
    DailyDigest,
    ApprovalEscalation,
    NotificationMail,
    RunReclaim,
    DailyBrief,
    McpLibraryRefresh,
    UpdateCheck,
    BlurbRewrite,
}

impl JobName {
    pub fn as_str(self) -> &'static str {
        match self {
            JobName::CommsDecay => "comms-decay",
            JobName::OutreachSweep => "outreach-sweep",
            JobName::PriceRefresh => "price-refresh",
            JobName::DailyDigest => "daily-digest",
            JobName::ApprovalEscalation => "approval-escalation",
            JobName::NotificationMail => "notification-mail",
            JobName::RunReclaim => "run-reclaim",
            JobName::DailyBrief => "daily-brief",
            JobName::McpLibraryRefresh => "mcp-library-refresh",
            JobName::UpdateCheck => "update-check",
            JobName::BlurbRewrite => "blurb-rewrite",
        }
    }
}

/// The jobs whose absence is SILENCE, and so must fail the boot check loudly.
/// The two TS optional jobs (mcp-library-refresh, update-check) are out for
/// their TS reasons: their failure mode is a slower first load or a stale
/// "last checked" next to a switch the panel shows, not work that silently
/// never happens. BlurbRewrite is in for the reason above — a dark sweep in
/// every proxied install is exactly the criterion.
pub const REQUIRED_JOBS: &[JobName] = &[
    JobName::CommsDecay,
    JobName::OutreachSweep,
    JobName::PriceRefresh,
    JobName::DailyDigest,
    JobName::ApprovalEscalation,
    JobName::NotificationMail,
    JobName::RunReclaim,
    JobName::DailyBrief,
    JobName::BlurbRewrite,
];

/// The work. `Ok(Some(sentence))` is a short human line for the log,
/// `Ok(None)` is "nothing to do" (logged quietly), `Err(text)` reports
/// failure — do not swallow. A panic is caught and recorded as a failure
/// with `job panicked:` in front of it, so the schedule behind it survives.
pub type JobFn = Arc<dyn Fn() -> BoxFuture<'static, Result<Option<String>, String>> + Send + Sync>;

#[derive(Clone)]
pub struct JobSpec {
    pub name: JobName,
    /// How often to attempt the job, in ms. The scheduler owns this; callers
    /// do not.
    pub every_ms: u64,
    /// Wait this long after start before the first attempt, in ms. Staggers
    /// boot, and means a crash-looping instance never reaches a job that
    /// writes.
    pub first_run_delay_ms: Option<u64>,
    /// How long one run is expected to take, at the OUTSIDE. Two things read
    /// it, so it is a real declaration and not a lease knob: the Redis lease
    /// is held (and renewed) for this long, so a crashed instance's job
    /// becomes available to another instance after roughly this delay; and
    /// `unhealthy_jobs` calls a run that outlives it HUNG rather than slow. A
    /// `per_instance` job takes no lease and this is still the number that
    /// makes its hang visible, so set it honestly there too.
    pub max_run_ms: Option<u64>,
    /// Skip the Redis lease and run on EVERY instance, every interval. Narrow
    /// and load-bearing: set it only when the job's entire input lives inside
    /// this process (an in-memory queue). If a job reads or writes rows that
    /// another instance can also reach, it needs the lease.
    pub per_instance: bool,
    pub run: JobFn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStatus {
    pub name: JobName,
    pub every_ms: u64,
    /// The outside bound for ONE run (`max_run_ms`, or the default). Past it a
    /// run is not slow, it is stuck — see `unhealthy_jobs`.
    pub max_run_ms: u64,
    pub running: bool,
    /// How long the CURRENT run has been going, in ms, or None when idle. The
    /// only field that can distinguish a wedged job from a healthy one.
    pub running_for_ms: Option<u64>,
    /// When the first run was due (ISO), or None when the scheduler never
    /// armed this job. Without it a job whose timer never fired has no
    /// "should have run by now" to be late against.
    pub first_run_due_at: Option<String>,
    pub runs: u64,
    pub failures: u64,
    /// Ticks skipped because the previous run was still going (this process).
    pub self_overlaps: u64,
    /// Ticks skipped because another instance held the lease, or Redis was
    /// down.
    pub lease_skips: u64,
    pub last_started_at: Option<String>,
    pub last_finished_at: Option<String>,
    pub last_duration_ms: Option<u64>,
    pub last_result: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthSeverity {
    Critical,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobHealth {
    pub name: JobName,
    pub severity: HealthSeverity,
    /// One line an operator can act on.
    pub detail: String,
}

// ── State ────────────────────────────────────────────────────────────────────

/// The mutable half of a job. Kept in its own small cell so the registry lock
/// is only ever held for synchronous, non-panicking assignments — never across
/// an await, and never around the job's own code. Poisoning-tolerant on
/// principle: a stuck counter must never make `scheduler_status` itself panic.
#[derive(Default)]
struct JobCell {
    running: bool,
    runs: u64,
    failures: u64,
    self_overlaps: u64,
    lease_skips: u64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    duration_ms: Option<u64>,
    result: Option<String>,
    error: Option<String>,
    armed_at: Option<i64>,
    first_run_due_at: Option<i64>,
}

struct RegisteredJob {
    spec: JobSpec,
    cell: Arc<Mutex<JobCell>>,
    /// The timer task: sleeps the first-run delay, then loops tick-then-sleep.
    /// Aborted on stop — that is what stops new runs being STARTED.
    task: Option<JoinHandle<()>>,
    /// The most recent in-flight attempt, if one is running. NOT aborted on
    /// stop — in-flight work is waited for, the way a redeploy should. The
    /// overlap guard means at most one attempt per job runs at a time in this
    /// process, so "the most recent" is also "the only".
    in_flight: Option<JoinHandle<()>>,
    /// How this job's lease gets renewed while it runs. None until armed, and
    /// None forever for a `per_instance` job (there is no lease to renew).
    heartbeat: Option<HeartbeatFactory>,
}

/// Builds the renewal loop for a lease an attempt just took. A factory because
/// the token only exists after the acquire; one is made per job at arm time
/// with the job's name in its log sentences.
pub type HeartbeatFactory = Arc<dyn Fn(LeaseToken, u64) -> LeaseHeartbeat + Send + Sync>;

static REGISTRY: LazyLock<Mutex<Vec<RegisteredJob>>> = LazyLock::new(|| Mutex::new(Vec::new()));

struct Ctl {
    started: bool,
    stopping: bool,
    conn: Option<redis::aio::ConnectionManager>,
}

static CTL: LazyLock<Mutex<Ctl>> = LazyLock::new(|| {
    Mutex::new(Ctl {
        started: false,
        stopping: false,
        conn: None,
    })
});

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Tolerant of poisoning on purpose: these cells are only ever locked for
/// plain assignments, so poisoning means somebody else's panic — and hiding
/// the counters would be a second failure stacked on the first.
fn lock_cell(cell: &Mutex<JobCell>) -> std::sync::MutexGuard<'_, JobCell> {
    cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The registration step, as a function over any list — so the
/// double-registration rule is testable without touching the shared registry.
fn push_job(jobs: &mut Vec<RegisteredJob>, spec: JobSpec) {
    if jobs.iter().any(|j| j.spec.name == spec.name) {
        // Re-registration is a real bug (two modules claiming one name, or a
        // module evaluated twice), and a silently doubled job is one of the
        // ways "it sent the message twice" happens. Keep the first, say so.
        tracing::error!(
            "{LOG} job \"{}\" registered twice — keeping the first registration. This is a bug.",
            spec.name.as_str()
        );
        return;
    }
    jobs.push(RegisteredJob {
        spec,
        cell: Arc::new(Mutex::new(JobCell::default())),
        task: None,
        in_flight: None,
        heartbeat: None,
    });
}

/// Declare a periodic job. Called by the module that owns the work — the
/// cadence lives next to the thing being scheduled, and the call is what puts
/// the job in the runtime graph.
pub fn register_job(spec: JobSpec) {
    let mut jobs = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    push_job(&mut jobs, spec);
}

/// The required jobs a list of registered names is missing — a fact about
/// this BUILD (a module fell out of the graph), not about whether this
/// instance is allowed to run anything.
fn missing_from(registered: &[JobName]) -> Vec<JobName> {
    REQUIRED_JOBS
        .iter()
        .filter(|n| !registered.contains(n))
        .copied()
        .collect()
}

/// The kill switch, as a pure read so a test can drive it without mutating
/// process env under parallel tests.
fn disabled_by_env(value: Option<&str>) -> bool {
    value == Some("off")
}

/// THE FLIP as a pure read — `TALARIA_SCHEDULER=rust` means this process owns
/// the schedule (see the header). Every behavior that changes at the handoff
/// reads this one predicate, so there is exactly one sentence to get right:
/// `work_dispatch::dispatch_deps` builds the real driver edges, the four
/// enqueue sites drive inline again, the models route's blurb kick stands
/// down for the blurb job, and `main` arms.
fn schedule_owned_here(value: Option<&str>) -> bool {
    value == Some("rust")
}

/// The env-read half of the flip predicate.
pub fn rust_owns_schedule() -> bool {
    schedule_owned_here(std::env::var("TALARIA_SCHEDULER").ok().as_deref())
}

fn job_lease_key(name: JobName) -> String {
    lease_key(SCHED_LEASE_NS, name.as_str())
}

/// Who is holding a lease we failed to take — for the log line only. "This
/// instance" means we already ran this interval and the key is cooling down;
/// that reading is true ONLY because of this file's demote-on-completion
/// policy — which is why the sentence is written here and the primitive
/// answers with a bare self/other.
async fn job_lease_holder(backend: &mut dyn LeaseBackend, name: JobName) -> Option<String> {
    match lease::lease_holder(backend, &job_lease_key(name)).await? {
        LeaseHolder::SelfHeld => Some("this instance already ran it this interval".into()),
        LeaseHolder::Other => Some("another instance holds it".into()),
    }
}

// ── One attempt ──────────────────────────────────────────────────────────────

/// What one attempt needs from the outside world. Production fills all of it
/// from the armed scheduler; the tests drive the backend directly, skip the
/// heartbeat (its beat is `renewal_beat`, already unit-tested in lease.rs) and
/// freeze the clock by capturing a copy of it here.
struct AttemptCtx<'a> {
    pub backend: &'a mut dyn LeaseBackend,
    pub heartbeat: Option<&'a HeartbeatFactory>,
    /// Read once at the top: a stop in flight turns away new attempts.
    pub stopping: bool,
    /// The clock, injectable for the same reason the backend is.
    pub now: Box<dyn Fn() -> i64 + Send>,
}

fn panic_text(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

/// Run one tick of a job. TOTAL: it resolves whatever happens, so a timer can
/// never leave a dangling task behind, and a failing job never stops its own
/// schedule. Nothing is swallowed — every branch that gives up says why.
async fn run_attempt(spec: &JobSpec, cell: Arc<Mutex<JobCell>>, ctx: AttemptCtx<'_>) {
    let name = spec.name.as_str();

    if ctx.stopping {
        return;
    }

    // Overlap guard, in-process. A job slower than its own interval must not
    // stack up: report the skip with how long the current run has been going,
    // which is the number you need to decide whether the interval is wrong.
    {
        let mut c = lock_cell(&cell);
        if c.running {
            c.self_overlaps += 1;
            let skips = c.self_overlaps;
            let for_ms = c.started_at.map(|s| (ctx.now)() - s).unwrap_or(0);
            tracing::warn!(
                "{LOG} {name} skipped: previous run still going after {for_ms}ms (skips={skips})"
            );
            return;
        }
    }

    let ttl_ms = spec.max_run_ms.unwrap_or(DEFAULT_MAX_RUN_MS).max(5_000);

    // A per_instance job's input is in THIS process's memory, so there is
    // nothing for another instance to duplicate and nothing for a lease to
    // protect — and taking one would stop every other instance draining its
    // own queue. It also means such a job keeps working when Redis is down,
    // which is right: it is not touching anything Redis is guarding.
    let lease = if spec.per_instance {
        None
    } else {
        match lease::acquire_lease(ctx.backend, &job_lease_key(spec.name), ttl_ms).await {
            AcquireResult::Acquired(token) => Some(token),
            AcquireResult::Unavailable(error) => {
                // Fail CLOSED. Running unguarded is how the same chat gets
                // archived twice and the same person gets DMed twice.
                let skips = {
                    let mut c = lock_cell(&cell);
                    c.lease_skips += 1;
                    c.lease_skips
                };
                tracing::error!(
                    "{LOG} {name} skipped: Redis lease unavailable (skips={skips}): {error}"
                );
                return;
            }
            AcquireResult::Held => {
                // Either someone is running it, or someone already ran it this
                // period. One extra round trip to say which, in the log only.
                let skips = {
                    let mut c = lock_cell(&cell);
                    c.lease_skips += 1;
                    c.lease_skips
                };
                let holder = job_lease_holder(ctx.backend, spec.name)
                    .await
                    .unwrap_or_else(|| "holder unknown".into());
                tracing::info!(
                    "{LOG} {name} skipped (skips={skips}): the lease for this interval is taken ({holder})"
                );
                return;
            }
        }
    };

    let started_at = (ctx.now)();
    {
        let mut c = lock_cell(&cell);
        c.running = true;
        c.started_at = Some(started_at);
        c.error = None;
    }

    // Keep the lease alive while the job runs, so work that legitimately
    // takes longer than one TTL is not stolen mid-flight. (A leased job with
    // no heartbeat factory — the tests — simply is not renewed.)
    let renew = lease
        .as_ref()
        .zip(ctx.heartbeat)
        .map(|(token, factory)| factory(token.clone(), ttl_ms));

    // TOTAL BY CONSTRUCTION: a panic in the job's future is caught here and
    // becomes this run's failure — if it escaped, the cleanup below would
    // never run, `running` would stay true forever, and the overlap guard
    // would turn away every tick behind it: the M1 wedge, rebuilt.
    let outcome = AssertUnwindSafe((spec.run)())
        .catch_unwind()
        .await
        .unwrap_or_else(|panic| Err(format!("job panicked: {}", panic_text(panic))));

    match outcome {
        Ok(result) => {
            let (ms, sentence) = {
                let mut c = lock_cell(&cell);
                c.runs += 1;
                c.result = result.clone();
                c.duration_ms = Some(((ctx.now)() - started_at).max(0) as u64);
                (c.duration_ms.unwrap_or(0), result)
            };
            match &sentence {
                Some(line) => tracing::info!("{LOG} {name} ok in {ms}ms — {line}"),
                None => tracing::info!("{LOG} {name} ok in {ms}ms — nothing to do"),
            }
        }
        Err(error) => {
            // The whole point of the file: a background failure that nobody
            // ever sees is the same as the work never happening. Name the
            // job, the elapsed time and the error, and keep it in status.
            let (ms, failures) = {
                let mut c = lock_cell(&cell);
                c.failures += 1;
                c.error = Some(error.clone());
                c.duration_ms = Some(((ctx.now)() - started_at).max(0) as u64);
                (c.duration_ms.unwrap_or(0), c.failures)
            };
            tracing::error!("{LOG} {name} FAILED after {ms}ms (failures={failures}): {error}");
        }
    }

    if let Some(renew) = renew {
        renew.stop();
    }

    // Hold the key for the rest of the interval rather than deleting it: that
    // is what makes this "once per interval, fleet-wide" instead of merely
    // "not at the same moment". Anchored to when the run STARTED, not when it
    // finished — a hold measured from the end pushes every period out by the
    // run's own duration, and the cadence drifts a little further behind on
    // every pass. Slightly short of a full interval (0.9) so a tick that
    // arrives a few ms early is not deferred a whole period.
    //
    // A `Lost` outcome here is deliberately silent: it means the run outlived
    // its own TTL and the lease has already gone, which the renewal loop
    // above has said out loud once already — and the next tick will simply
    // find the key free and run, which is the correct recovery.
    let hold_ms =
        (((started_at + (spec.every_ms as f64 * 0.9).floor() as i64) - (ctx.now)()).max(1)) as u64;
    if let Some(token) = &lease {
        match lease::demote_lease(ctx.backend, token, hold_ms).await {
            lease::LeaseResult::Unavailable(error) => tracing::error!(
                "{LOG} {name} could not hold its lease for the rest of the interval: {error}"
            ),
            lease::LeaseResult::Ok | lease::LeaseResult::Lost => {}
        }
    }

    // Cleared LAST, so a tick that arrives between the run ending and the
    // lease being demoted is turned away by the cheap in-process guard rather
    // than by a Redis round trip.
    {
        let mut c = lock_cell(&cell);
        c.running = false;
        c.finished_at = Some((ctx.now)());
    }
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

/// One timer fire: spawn the attempt as its own task so a long run can never
/// block the tick loop, and record the handle so `stop_scheduler` can wait
/// for whatever is in flight. The name — not an index — is what the timer
/// task holds, so registration order can never drift a tick onto the wrong
/// job.
fn tick_now(name: JobName) {
    let (spec, cell, heartbeat) = {
        let jobs = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
        let Some(job) = jobs.iter().find(|j| j.spec.name == name) else {
            return;
        };
        (job.spec.clone(), job.cell.clone(), job.heartbeat.clone())
    };
    let (stopping, conn) = {
        let ctl = CTL.lock().unwrap_or_else(|p| p.into_inner());
        (ctl.stopping, ctl.conn.clone())
    };
    let Some(conn) = conn else {
        tracing::error!(
            "{LOG} {} tick arrived before the scheduler was armed — arming is what builds the lease connection",
            spec.name.as_str()
        );
        return;
    };
    let handle = tokio::spawn(async move {
        let mut backend = RedisLeases::new(conn);
        let ctx = AttemptCtx {
            backend: &mut backend,
            heartbeat: heartbeat.as_ref(),
            stopping,
            now: Box::new(now_ms),
        };
        run_attempt(&spec, cell, ctx).await;
    });
    let mut jobs = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(job) = jobs.iter_mut().find(|j| j.spec.name == name) {
        job.in_flight = Some(handle);
    }
}

/// Start every registered job. Idempotent. Returns the names actually armed.
///
/// Not armed in dev: `main` calls it only when the flip flag is set, the same
/// way TS's boot path is the only thing that calls `startScheduler`.
pub fn start_scheduler(conn: redis::aio::ConnectionManager) -> Vec<JobName> {
    // BEFORE the kill switch, deliberately. A required job that never
    // registered means its module was not in the runtime graph — a fact about
    // this BUILD, not about whether this instance is allowed to run anything.
    // Checking it after the `off` branch means the one check that catches "a
    // job fell out of the graph" goes silent precisely when an operator is
    // running with the switch thrown to work out what this deployment is
    // doing. Loudly, because the symptom otherwise is "the digest just never
    // arrives" months later.
    let missing = missing_from(&registered_names());
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|n| n.as_str()).collect();
        tracing::error!(
            "{LOG} MISSING JOBS: {} did not register. Their module was never imported, so that work will NOT run. This is a bug.",
            names.join(", ")
        );
    }

    if disabled_by_env(std::env::var("TALARIA_SCHEDULER").ok().as_deref()) {
        tracing::warn!(
            "{LOG} disabled by TALARIA_SCHEDULER=off — no background jobs will run on this instance"
        );
        return Vec::new();
    }

    {
        let mut ctl = CTL.lock().unwrap_or_else(|p| p.into_inner());
        if ctl.started {
            tracing::warn!("{LOG} start_scheduler() called twice — ignoring the second call");
            return registered_names();
        }
        ctl.started = true;
        ctl.stopping = false;
        ctl.conn = Some(conn.clone());
    }

    let mut armed: Vec<(JobName, u64)> = Vec::new();
    {
        let mut jobs = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
        let boot = now_ms();
        for job in jobs.iter_mut() {
            let name = job.spec.name;
            let delay = job.spec.first_run_delay_ms.unwrap_or(0);
            let every = job.spec.every_ms;
            {
                let mut c = lock_cell(&job.cell);
                c.armed_at = Some(boot);
                c.first_run_due_at = Some(boot + delay as i64);
            }
            // The heartbeat factory, made here where the connection and the
            // job's name are both in hand. Its two callbacks are the two
            // sentences a renewal can produce, with the job named.
            job.heartbeat = if job.spec.per_instance {
                None
            } else {
                let beat_conn = conn.clone();
                let beat_name = name.as_str();
                Some(Arc::new(move |token, ttl| {
                    keep_lease_alive(
                        beat_conn.clone(),
                        token,
                        ttl,
                        HeartbeatOptions {
                            every_ms: None,
                            on_lost: Some(Arc::new(move || {
                                tracing::warn!(
                                    "{LOG} {beat_name} lost its lease while running — another instance may have started it"
                                )
                            })),
                            on_error: Some(Arc::new(move |e| {
                                tracing::error!("{LOG} {beat_name} lease renewal failed: {e}")
                            })),
                        },
                    )
                }))
            };
            // One task per job: sleep the first-run delay, then tick and
            // sleep forever. The spawn is safe under the registry lock — the
            // task sleeps before its first tick, so it cannot need the lock
            // until this block has dropped it.
            let handle = tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                loop {
                    tick_now(name);
                    tokio::time::sleep(Duration::from_millis(every)).await;
                }
            });
            job.task = Some(handle);
            armed.push((name, every));
        }
    }

    let summary: Vec<String> = armed
        .iter()
        .map(|(name, every)| format!("{} every {}s", name.as_str(), every / 1000))
        .collect();
    tracing::info!(
        "{LOG} started on instance {} — {} job(s): {}",
        instance_id(),
        armed.len(),
        summary.join(", ")
    );
    armed.into_iter().map(|(name, _)| name).collect()
}

fn registered_names() -> Vec<JobName> {
    REGISTRY
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|j| j.spec.name)
        .collect()
}

/// Clear every timer and wait (briefly) for anything in flight. Called on
/// shutdown so a redeploy stops arming new runs the moment it is told to go,
/// instead of being killed mid-archive.
pub async fn stop_scheduler(grace_ms: u64) {
    {
        let mut ctl = CTL.lock().unwrap_or_else(|p| p.into_inner());
        if !ctl.started {
            return;
        }
        ctl.stopping = true;
        ctl.started = false;
    }
    let in_flight: Vec<JoinHandle<()>> = {
        let mut jobs = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
        jobs.iter_mut()
            .flat_map(|j| j.task.take())
            .for_each(|timer| timer.abort());
        jobs.iter_mut().filter_map(|j| j.in_flight.take()).collect()
    };
    if in_flight.is_empty() {
        tracing::info!("{LOG} stopped — no job was in flight");
        CTL.lock().unwrap_or_else(|p| p.into_inner()).stopping = false;
        return;
    }
    tracing::info!(
        "{LOG} stopping — waiting up to {grace_ms}ms for {} job(s) in flight",
        in_flight.len()
    );
    match tokio::time::timeout(Duration::from_millis(grace_ms), join_all(in_flight)).await {
        Ok(_) => tracing::info!("{LOG} stopped cleanly"),
        Err(_) => tracing::warn!(
            "{LOG} stopped with job(s) still in flight after {grace_ms}ms — their Redis lease will expire on its own"
        ),
    }
    CTL.lock().unwrap_or_else(|p| p.into_inner()).stopping = false;
}

pub fn scheduler_status(now: i64) -> Vec<JobStatus> {
    let jobs = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    jobs.iter()
        .map(|j| status_of(&j.spec, &lock_cell(&j.cell), now))
        .collect()
}

/// The one place a `JobCell` becomes readable state — shared by
/// `scheduler_status` and the tests, so the health walk's inputs and the
/// operator's view of them cannot drift apart.
fn status_of(spec: &JobSpec, cell: &JobCell, now: i64) -> JobStatus {
    JobStatus {
        name: spec.name,
        every_ms: spec.every_ms,
        max_run_ms: spec.max_run_ms.unwrap_or(DEFAULT_MAX_RUN_MS),
        running: cell.running,
        running_for_ms: cell
            .running
            .then_some((now - cell.started_at.unwrap_or(now)).max(0) as u64),
        first_run_due_at: cell.first_run_due_at.map(epoch_ms_to_iso),
        runs: cell.runs,
        failures: cell.failures,
        self_overlaps: cell.self_overlaps,
        lease_skips: cell.lease_skips,
        last_started_at: cell.started_at.map(epoch_ms_to_iso),
        last_finished_at: cell.finished_at.map(epoch_ms_to_iso),
        last_duration_ms: cell.duration_ms,
        last_result: cell.result.clone(),
        last_error: cell.error.clone(),
    }
}

// ── Health, in sentences ─────────────────────────────────────────────────────

/// Every registered job that is not doing its job, and why.
///
/// PROCESS-LOCAL, and that is not a bug to fix here: the counters live in
/// this process's memory, so on a multi-instance deployment this describes
/// whichever instance answered the request. That still surfaces a job failing
/// everywhere (every instance reports it) and a job failing on one box (it
/// appears intermittently), which is the difference an operator needs.
pub fn unhealthy_jobs(now: i64) -> Vec<JobHealth> {
    let started = CTL.lock().unwrap_or_else(|p| p.into_inner()).started;
    health_from(&scheduler_status(now), started, now)
}

/// Five things count as unhealthy, in the order they matter: the last run
/// threw; the current run has outlived `max_run_ms` (HUNG, not slow); the job
/// was armed and its first run never landed at all; the job has not completed
/// a run within two intervals of when it should have; and the job keeps
/// skipping itself because a run outlives its own interval — the schedule
/// being wrong rather than the code being broken. The middle two are the M1
/// cold-boot cases: they fire on a job whose counters are all ZERO, which is
/// why they look at time and not at failures.
fn health_from(statuses: &[JobStatus], started: bool, now: i64) -> Vec<JobHealth> {
    let mut out = Vec::new();
    // Operator-facing, so say seconds when it is seconds: "past the 1-minute
    // bound" for a 45s bound is the kind of rounding that makes someone stop
    // trusting the sentence.
    let dur = |ms: u64| {
        if ms < 90_000 {
            format!("{}s", (ms / 1_000).max(1))
        } else {
            format!("{} minutes", ms / 60_000)
        }
    };
    for s in statuses {
        if let Some(error) = &s.last_error {
            out.push(JobHealth {
                severity: HealthSeverity::Critical,
                name: s.name,
                detail: format!(
                    "the last run failed after {}ms: {} ({} failure{} since boot). This work is not happening.",
                    s.last_duration_ms.unwrap_or(0),
                    error,
                    s.failures,
                    if s.failures == 1 { "" } else { "s" }
                ),
            });
            continue;
        }
        // HUNG. `max_run_ms` is the job's own declared outside bound for one
        // run, so past it the run is not slow — it is never coming back, and
        // because the overlap guard is doing its job the schedule behind it
        // is stopped dead.
        if s.running && s.running_for_ms.is_some_and(|ms| ms > s.max_run_ms) {
            out.push(JobHealth {
                severity: HealthSeverity::Critical,
                name: s.name,
                detail: format!(
                    "has been running for {} — past the {} bound it declares for one run. It is not coming back on its own, and the schedule behind it is stopped ({} tick(s) skipped since). This work is not happening.",
                    dur(s.running_for_ms.unwrap_or(0)),
                    dur(s.max_run_ms),
                    s.self_overlaps
                ),
            });
            continue;
        }
        // The remaining three are schedule facts, so they are only meaningful
        // once the scheduler is actually armed — a registered-but-unarmed job
        // has never been asked to run anything and must not be reported as
        // late.
        if started {
            let last_ms = s
                .last_finished_at
                .as_deref()
                .or(s.last_started_at.as_deref())
                .and_then(iso_to_epoch_ms);
            // Armed, due, and NOTHING — not even a start. The timer never
            // fired, or it fired into something that never reached an
            // attempt. There is no `last` to be late against, which is
            // exactly why this needs its own case.
            if last_ms.is_none()
                && let Some(due_ms) = s.first_run_due_at.as_deref().and_then(iso_to_epoch_ms)
            {
                let overdue_ms = now - due_ms;
                if overdue_ms > (s.every_ms as i64) * 2 {
                    out.push(JobHealth {
                        severity: HealthSeverity::Critical,
                        name: s.name,
                        detail: format!(
                            "was armed at boot and has never started a run — its first was due {} ago, on a {} schedule.",
                            dur(overdue_ms.max(0) as u64),
                            dur(s.every_ms)
                        ),
                    });
                    continue;
                }
            }
            if !s.running
                && let Some(last_ms) = last_ms
                && (now - last_ms) > (s.every_ms as i64) * 2
            {
                let since_ms = now - last_ms;
                out.push(JobHealth {
                    severity: HealthSeverity::Warning,
                    name: s.name,
                    detail: format!(
                        "has not completed a run for {}, on a {} schedule.",
                        dur(since_ms.max(0) as u64),
                        dur(s.every_ms)
                    ),
                });
                continue;
            }
        }
        if s.runs > 0 && s.self_overlaps > s.runs {
            out.push(JobHealth {
                severity: HealthSeverity::Warning,
                name: s.name,
                detail: format!(
                    "skipped {} ticks because the previous run was still going ({} completed). The interval is shorter than the job takes.",
                    s.self_overlaps, s.runs
                ),
            });
        }
    }
    // Critical first; within a severity, the registration order the status
    // walk produced (the sort is stable, matching TS's comparator).
    out.sort_by(|a, b| match (a.severity, b.severity) {
        (HealthSeverity::Critical, HealthSeverity::Warning) => std::cmp::Ordering::Less,
        (HealthSeverity::Warning, HealthSeverity::Critical) => std::cmp::Ordering::Greater,
        _ => std::cmp::Ordering::Equal,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An in-memory Redis with real expiry semantics on a manually
    /// advanceable clock — the same shape the lease primitive's own tests
    /// drive, so the scheduler's POLICY is testable with no server, no
    /// timers and no sleeping. The clock is frozen during an attempt by
    /// capturing a copy of it into the AttemptCtx, which is exactly the
    /// contract production has: `now` is read, never assumed stale.
    struct FakeRedis {
        now_ms: i64,
        map: HashMap<String, (String, i64)>,
        fail: bool,
    }

    impl FakeRedis {
        fn new() -> Self {
            Self {
                now_ms: 10_000,
                map: HashMap::new(),
                fail: false,
            }
        }
        fn live(&self, key: &str) -> Option<&(String, i64)> {
            self.map.get(key).filter(|(_, exp)| *exp > self.now_ms)
        }
    }

    impl LeaseBackend for FakeRedis {
        fn set_nx_px<'a>(
            &'a mut self,
            key: &'a str,
            value: &'a str,
            ttl_ms: u64,
        ) -> BoxFuture<'a, Result<bool, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("connection dropped".into());
                }
                if self.live(key).is_some() {
                    return Ok(false);
                }
                self.map.insert(
                    key.to_string(),
                    (
                        value.to_string(),
                        self.now_ms + lease::clamp_ttl(ttl_ms) as i64,
                    ),
                );
                Ok(true)
            })
        }
        fn pexpire_if_eq<'a>(
            &'a mut self,
            key: &'a str,
            value: &'a str,
            ttl_ms: u64,
        ) -> BoxFuture<'a, Result<bool, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("connection dropped".into());
                }
                if self.live(key).map(|(v, _)| v.as_str()) == Some(value) {
                    let slot = self.map.get_mut(key).unwrap();
                    slot.1 = self.now_ms + lease::clamp_ttl(ttl_ms) as i64;
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        }
        fn del_if_eq<'a>(
            &'a mut self,
            key: &'a str,
            value: &'a str,
        ) -> BoxFuture<'a, Result<bool, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("connection dropped".into());
                }
                if self.live(key).map(|(v, _)| v.as_str()) == Some(value) {
                    self.map.remove(key);
                    Ok(true)
                } else {
                    Ok(false)
                }
            })
        }
        fn get<'a>(&'a mut self, key: &'a str) -> BoxFuture<'a, Result<Option<String>, String>> {
            Box::pin(async move {
                if self.fail {
                    return Err("connection dropped".into());
                }
                Ok(self.live(key).cloned().map(|(v, _)| v))
            })
        }
    }

    fn spec(name: JobName, every_ms: u64) -> JobSpec {
        JobSpec {
            name,
            every_ms,
            first_run_delay_ms: None,
            max_run_ms: Some(5_000),
            per_instance: false,
            run: Arc::new(|| Box::pin(async { Ok(None) })),
        }
    }

    fn ctx<'a>(backend: &'a mut FakeRedis, stopping: bool) -> AttemptCtx<'a> {
        let now = backend.now_ms;
        AttemptCtx {
            backend,
            heartbeat: None,
            stopping,
            now: Box::new(move || now),
        }
    }

    #[tokio::test]
    async fn a_successful_run_counts_and_spends_the_period() {
        let mut redis = FakeRedis::new();
        let cell = Arc::new(Mutex::new(JobCell::default()));
        let spec = JobSpec {
            run: Arc::new(|| Box::pin(async { Ok(Some("archived 3 conversations".into())) })),
            ..spec(JobName::PriceRefresh, 60_000)
        };
        run_attempt(&spec, cell.clone(), ctx(&mut redis, false)).await;
        {
            let c = lock_cell(&cell);
            assert_eq!(c.runs, 1);
            assert!(!c.running);
            assert_eq!(c.result.as_deref(), Some("archived 3 conversations"));
            assert!(c.error.is_none());
        }
        // The period is spent: the key is still there, held to just under the
        // next interval (0.9 × every, anchored to the START), so a second tick
        // in the SAME period is turned away.
        let key = job_lease_key(JobName::PriceRefresh);
        assert!(redis.live(&key).is_some());
        let expiry = redis.map[&key].1;
        assert!(
            (expiry - 10_000 - 54_000).abs() <= 1,
            "hold should be ~0.9×every from the start, got expiry {expiry}"
        );
        run_attempt(&spec, cell.clone(), ctx(&mut redis, false)).await;
        let c = lock_cell(&cell);
        assert_eq!(c.lease_skips, 1, "the demoted key turns the next tick away");
        assert_eq!(c.runs, 1, "one run per interval, not two");
    }

    #[tokio::test]
    async fn a_failing_run_is_recorded_not_swallowed() {
        let mut redis = FakeRedis::new();
        let cell = Arc::new(Mutex::new(JobCell::default()));
        let spec = JobSpec {
            run: Arc::new(|| Box::pin(async { Err("the sweep query timed out".into()) })),
            ..spec(JobName::OutreachSweep, 60_000)
        };
        run_attempt(&spec, cell.clone(), ctx(&mut redis, false)).await;
        let c = lock_cell(&cell);
        assert_eq!(c.failures, 1);
        assert_eq!(c.error.as_deref(), Some("the sweep query timed out"));
        assert!(!c.running, "the finally path clears running on failure too");
        // A failed run still spends the period — the work was attempted.
        assert!(redis.live(&job_lease_key(JobName::OutreachSweep)).is_some());
    }

    #[tokio::test]
    async fn a_panic_is_a_failure_and_never_wedges_the_guard() {
        // THE M1 LESSON, in one test: a job that neither fails nor returns
        // used to leave `running` stuck true while every counter stayed zero,
        // and the overlap guard turned away every tick behind it. A panic is
        // Rust's version of that job.
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let mut redis = FakeRedis::new();
        let cell = Arc::new(Mutex::new(JobCell::default()));
        let spec = JobSpec {
            run: Arc::new(|| Box::pin(async { panic!("the digest template exploded") })),
            ..spec(JobName::DailyDigest, 60_000)
        };
        run_attempt(&spec, cell.clone(), ctx(&mut redis, false)).await;
        std::panic::set_hook(prev_hook);
        let c = lock_cell(&cell);
        assert_eq!(c.failures, 1);
        assert_eq!(
            c.error.as_deref(),
            Some("job panicked: the digest template exploded")
        );
        assert!(!c.running, "the guard is clear for the next tick");
    }

    #[tokio::test]
    async fn an_overlapping_tick_is_turned_away_by_the_process_guard() {
        let mut redis = FakeRedis::new();
        let cell = Arc::new(Mutex::new(JobCell::default()));
        {
            let mut c = lock_cell(&cell);
            c.running = true;
            c.started_at = Some(2_000);
        }
        run_attempt(
            &spec(JobName::CommsDecay, 60_000),
            cell.clone(),
            ctx(&mut redis, false),
        )
        .await;
        let c = lock_cell(&cell);
        assert_eq!(c.self_overlaps, 1);
        assert_eq!(c.runs, 0);
        assert!(
            redis.map.is_empty(),
            "the in-process guard answers before any Redis round trip"
        );
    }

    #[tokio::test]
    async fn an_unreachable_redis_skips_the_tick_fail_closed() {
        let mut redis = FakeRedis {
            fail: true,
            ..FakeRedis::new()
        };
        let cell = Arc::new(Mutex::new(JobCell::default()));
        run_attempt(
            &spec(JobName::NotificationMail, 60_000),
            cell.clone(),
            ctx(&mut redis, false),
        )
        .await;
        let c = lock_cell(&cell);
        assert_eq!(c.lease_skips, 1);
        assert_eq!(
            c.runs, 0,
            "a skipped tick runs nothing — a duplicate DM is unrecoverable"
        );
    }

    #[tokio::test]
    async fn a_lease_held_by_another_instance_skips_the_tick() {
        let mut redis = FakeRedis::new();
        redis.map.insert(
            job_lease_key(JobName::RunReclaim),
            ("someone-else:token".into(), 100_000),
        );
        let cell = Arc::new(Mutex::new(JobCell::default()));
        run_attempt(
            &spec(JobName::RunReclaim, 60_000),
            cell.clone(),
            ctx(&mut redis, false),
        )
        .await;
        let c = lock_cell(&cell);
        assert_eq!(c.lease_skips, 1);
        assert_eq!(c.runs, 0);
        assert_eq!(
            redis.map[&job_lease_key(JobName::RunReclaim)].0,
            "someone-else:token",
            "a held key is left exactly as the other instance wrote it"
        );
    }

    #[tokio::test]
    async fn a_per_instance_job_never_touches_the_lease() {
        let mut redis = FakeRedis::new();
        let cell = Arc::new(Mutex::new(JobCell::default()));
        let spec = JobSpec {
            per_instance: true,
            ..spec(JobName::UpdateCheck, 60_000)
        };
        run_attempt(&spec, cell.clone(), ctx(&mut redis, false)).await;
        assert_eq!(lock_cell(&cell).runs, 1);
        assert!(
            redis.map.is_empty(),
            "an in-process queue has nothing for a lease to protect"
        );
    }

    #[tokio::test]
    async fn a_stopping_scheduler_turns_away_new_attempts() {
        let mut redis = FakeRedis::new();
        let cell = Arc::new(Mutex::new(JobCell::default()));
        run_attempt(
            &spec(JobName::DailyBrief, 60_000),
            cell.clone(),
            ctx(&mut redis, true),
        )
        .await;
        assert_eq!(lock_cell(&cell).runs, 0);
        assert!(redis.map.is_empty());
    }

    #[test]
    fn the_kill_switch_reads_only_its_own_value() {
        assert!(disabled_by_env(Some("off")));
        assert!(!disabled_by_env(Some("on")));
        assert!(!disabled_by_env(None));
    }

    #[test]
    fn the_flip_value_is_rust_and_only_rust() {
        // Exactly one spelling moves the schedule, and the two other
        // postures do not: 'off' disables both runtimes, unset leaves it
        // with TS — so a typo in an env file can never half-flip a
        // deployment into both-armed or neither-armed.
        assert!(schedule_owned_here(Some("rust")));
        assert!(!schedule_owned_here(Some("off")));
        assert!(!schedule_owned_here(None));
        assert!(!schedule_owned_here(Some("Rust")));
        assert!(!schedule_owned_here(Some("on")));
    }

    #[test]
    fn the_required_table_is_the_ts_eight_plus_the_new_sweep() {
        let names: Vec<&str> = REQUIRED_JOBS.iter().map(|n| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "comms-decay",
                "outreach-sweep",
                "price-refresh",
                "daily-digest",
                "approval-escalation",
                "notification-mail",
                "run-reclaim",
                "daily-brief",
                "blurb-rewrite",
            ]
        );
    }

    #[test]
    fn the_missing_check_names_exactly_what_is_absent() {
        assert_eq!(missing_from(&[]).len(), REQUIRED_JOBS.len());
        let all: Vec<JobName> = REQUIRED_JOBS.to_vec();
        assert!(missing_from(&all).is_empty());
        // A non-required job registering changes nothing.
        assert_eq!(
            missing_from(&[JobName::UpdateCheck]).len(),
            REQUIRED_JOBS.len()
        );
    }

    #[test]
    fn re_registration_keeps_the_first() {
        let mut jobs: Vec<RegisteredJob> = Vec::new();
        push_job(&mut jobs, spec(JobName::McpLibraryRefresh, 60_000));
        push_job(&mut jobs, spec(JobName::McpLibraryRefresh, 120_000));
        assert_eq!(jobs.len(), 1, "the second registration is refused");
        assert_eq!(jobs[0].spec.every_ms, 60_000, "the FIRST registration wins");
    }

    #[test]
    fn health_names_a_failed_run_and_a_hung_one() {
        // Failed: the last run threw.
        let cell = JobCell {
            failures: 1,
            error: Some("the mail transport refused every recipient".into()),
            duration_ms: Some(2_400),
            ..Default::default()
        };
        let s = status_of(&spec(JobName::NotificationMail, 60_000), &cell, 100_000);
        let health = health_from(&[s], false, 100_000);
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].severity, HealthSeverity::Critical);
        assert!(
            health[0]
                .detail
                .contains("the mail transport refused every recipient")
        );
        assert!(health[0].detail.contains("1 failure since boot"));

        // HUNG: running, 45s in, past its own 5s bound — and the sentence
        // says seconds, not "1 minutes".
        let cell = JobCell {
            running: true,
            started_at: Some(55_000),
            ..Default::default()
        };
        let s = status_of(&spec(JobName::CommsDecay, 60_000), &cell, 100_000);
        let health = health_from(&[s], false, 100_000);
        assert_eq!(health.len(), 1);
        assert!(health[0].detail.contains("has been running for 45s"));
        assert!(health[0].detail.contains("past the 5s bound"));
        assert!(health[0].detail.contains("This work is not happening"));
    }

    #[test]
    fn health_reports_hangs_in_minutes_when_they_are_minutes() {
        let cell = JobCell {
            running: true,
            started_at: Some(0),
            ..Default::default()
        };
        let spec = JobSpec {
            max_run_ms: Some(10 * 60_000),
            ..spec(JobName::DailyDigest, 15 * 60_000)
        };
        let s = status_of(&spec, &cell, 20 * 60_000);
        let health = health_from(&[s], false, 20 * 60_000);
        assert_eq!(health.len(), 1);
        assert!(health[0].detail.contains("has been running for 20 minutes"));
        assert!(health[0].detail.contains("past the 10 minutes bound"));
    }

    #[test]
    fn health_names_a_job_that_never_started_and_one_that_went_quiet() {
        // Never started: armed, due long ago, no run at all. `started: true`
        // stands in for the armed scheduler.
        let cell = JobCell {
            first_run_due_at: Some(0),
            ..Default::default()
        };
        let s = status_of(&spec(JobName::DailyDigest, 60_000), &cell, 300_000);
        let health = health_from(&[s], true, 300_000);
        assert_eq!(health.len(), 1);
        assert!(health[0].detail.contains("has never started a run"));
        assert!(health[0].detail.contains("due 5 minutes ago"));

        // Quiet: last finished 300s ago on a 60s schedule.
        let cell = JobCell {
            runs: 4,
            finished_at: Some(0),
            ..Default::default()
        };
        let s = status_of(&spec(JobName::DailyDigest, 60_000), &cell, 300_000);
        let health = health_from(&[s], true, 300_000);
        assert_eq!(health.len(), 1);
        assert!(
            health[0]
                .detail
                .contains("has not completed a run for 5 minutes")
        );
        assert_eq!(health[0].severity, HealthSeverity::Warning);
    }

    #[test]
    fn an_unarmed_registry_is_never_reported_as_late() {
        // The same never-started cell, read while the scheduler is NOT armed:
        // a registered-but-unarmed job has never been asked to run anything.
        let cell = JobCell {
            first_run_due_at: Some(0),
            ..Default::default()
        };
        let s = status_of(&spec(JobName::DailyDigest, 60_000), &cell, 300_000);
        assert!(health_from(&[s], false, 300_000).is_empty());
    }

    #[test]
    fn health_names_a_schedule_shorter_than_its_job() {
        let cell = JobCell {
            runs: 3,
            self_overlaps: 9,
            finished_at: Some(90_000),
            ..Default::default()
        };
        let s = status_of(&spec(JobName::PriceRefresh, 60_000), &cell, 100_000);
        let health = health_from(&[s], false, 100_000);
        assert_eq!(health.len(), 1);
        assert!(
            health[0]
                .detail
                .contains("The interval is shorter than the job takes")
        );
        assert_eq!(health[0].severity, HealthSeverity::Warning);
    }

    #[test]
    fn critical_finds_sort_before_warnings() {
        let failed = JobCell {
            failures: 1,
            error: Some("boom".into()),
            ..Default::default()
        };
        let overlap = JobCell {
            runs: 1,
            self_overlaps: 5,
            ..Default::default()
        };
        let statuses = [
            status_of(&spec(JobName::PriceRefresh, 60_000), &overlap, 100_000),
            status_of(&spec(JobName::NotificationMail, 60_000), &failed, 100_000),
        ];
        let health = health_from(&statuses, false, 100_000);
        assert_eq!(health.len(), 2);
        assert_eq!(health[0].severity, HealthSeverity::Critical);
        assert_eq!(health[1].severity, HealthSeverity::Warning);
    }
}
