// The reclaim sweeper — the thing that makes "survives a restart" TRUE.
//
// WHAT IT REPLACES
//   server/research.ts:349 has a sweep with exactly this shape and exactly the
//   wrong body:
//
//     update research_runs set status = 'error',
//       error = 'run went stale (app restarted mid-research?)'
//
//   It does not resume the run. It gives up on it, forty-five minutes later,
//   and tells the person who asked for a research report that it failed — for
//   the crime of a deploy landing while they waited. Nothing was lost when that
//   process died: the work up to the last checkpoint was on disk and the next
//   step was re-enterable. That sentence is the bug this whole project exists
//   to delete, and this file is where it gets deleted: the same event (a driver
//   stopped renewing) produces a RESUME instead of an epitaph.
//
// WHAT IT DOES, in one pass
//   · asks the store for runs nobody is driving — `queued` with no live lease,
//     or `running` whose lease has EXPIRED — oldest expiry first;
//   · hands each one to `drive()`, which claims it (the claim is what bumps
//     `attempt` on a reclaim) and re-enters `step()` with the last PERSISTED
//     checkpoint;
//   · and does it BOUNDED, because the moment there are most orphans to recover
//     is the moment right after a deploy, which is also the moment an instance
//     is least able to absorb an unbounded fan-out of drives.
//
// WHAT IT MUST NEVER DO — and this is the one to hold on to. A run in
// `awaiting` IS NOT STALE, no matter how long it sits. It is parked on a
// question a person has not answered yet; that is a healthy state and it may
// last days. A sweeper that treated "old and not moving" as "broken" would
// auto-fail every paused run in the product, which is `research.ts`'s bug
// wearing a different hat — and worse, because the run would be destroyed while
// the notification asking about it was still in somebody's inbox. The guard is
// `is_drivable` (runs/define.rs, which says the same thing in its doc comment)
// and there is a test pinning it.
//
// WHO WRITES WHAT — the split with run.rs, stated once so nobody has to infer
// it. `drive()` is the ONLY code in the runs system that writes a state
// transition or publishes one; every write in store.rs is a compare-and-set on
// the lease token, and the driver holds that token. So this file writes
// NOTHING. It selects, it bounds, it hands over, and it reports. In particular
// it does not publish an event of its own: the transitions this sweep causes
// are published by the driver that performs them (a checkpoint, a park, a
// give-up), and an event from here would either duplicate one of those or —
// worse — announce a state the row is not in yet, because a sweep cannot know
// whether its kick will win the run's Redis lease.
//
// TESTABILITY IS A DESIGN CONSTRAINT. Every edge — the query, the registry, the
// driver, the clock — is a field on `ReclaimDeps`, so runs_reclaim.rs drives
// the whole sweeper with no database, no Redis and no clock.
//
// THE REGISTRATION is `reclaim_job_spec`/`register_reclaim_job` at the bottom
// of this file: the four numbers below in their four slots, not perInstance,
// armed by the flip's boot path (Rust's deps are runtime values, so the call
// — not the module load — is what puts the job in the runtime graph). Until
// the flip arms the scheduler, registering costs nothing and running nothing.
use std::sync::Arc;

use futures_util::future::BoxFuture;

use crate::runs::define::{RunRow, RunState, is_drivable};
use crate::runs::run::{DefinitionForFn, DriveResult, DriveStop, NowFn, RunDeps, drive};
use crate::runs::store::RunStore;

const LOG: &str = "[runs/reclaim]";

/// Operator-facing durations. Say seconds when it is seconds: "0 minutes" for a
/// twenty-second-old lease is the kind of rounding that makes somebody stop
/// reading the line.
///
/// TS rounds (`Math.round`); Rust truncates — they differ only inside the
/// discarded fraction of a minute or second, in a log line nothing parses.
fn dur(ms: i64) -> String {
    if ms < 90_000 {
        format!("{}s", (ms / 1_000).max(0))
    } else {
        format!("{} minutes", ms / 60_000)
    }
}

// ── The declared timings ─────────────────────────────────────────────────────
//
// Constants rather than numbers buried in a spec literal because two of them
// are promises the rest of the system reads: LIMIT is the recovery RATE (with
// EVERY_MS), and MAX_RUN_MS is what the scheduler's unhealthy-jobs check will
// use to call a wedged sweep hung rather than slow. A reviewer should be able
// to see all four numbers and the reasoning for each in one place.

pub const RECLAIM_JOB: &str = "run-reclaim";

/// HOW FAST A CRASHED RUN COMES BACK. A run's lease is its definition's
/// `max_step_ms`, so the row becomes reclaimable roughly one step after the
/// process holding it died; this interval is the rest of the delay a waiting
/// person sees. Thirty seconds makes the worst case "one step, plus half a
/// minute" — fast enough that a deploy reads as a pause rather than a stall,
/// and cheap enough to be free: one pass is a single partial-index scan
/// (runs_reclaim_idx), and the scheduler's lease means it happens once per
/// interval across the whole fleet, not once per instance.
pub const RECLAIM_EVERY_MS: u64 = 30_000;

/// Let the instance settle before it starts re-entering steps.
///
/// Shorter than comms-decay's two minutes because what this job resumes is work
/// a person is actively watching, and longer than zero for the reason every
/// first-run delay in this codebase exists: a crash-looping instance must never
/// reach a job that writes. And this one writes through other people's side
/// effects — a reclaimed step can bill a model call, open a PR or send a DM,
/// because the runtime is AT-LEAST-ONCE. A boot loop that swept immediately
/// would re-enter the same step on every restart.
pub const RECLAIM_FIRST_RUN_DELAY_MS: u64 = 20_000;

/// THE OUTSIDE BOUND FOR ONE PASS, and it is read: the scheduler's
/// unhealthy-jobs check calls a run that outlives it HUNG rather than slow,
/// which is the only way a sweeper that stopped coming back becomes visible to
/// anybody. That is the exact failure the scheduler header warns about — a job
/// that neither throws nor returns leaves `failures` at 0, `runs` at 0 and
/// `running` true forever — and it is a real risk here, because every await in
/// a pass is a database round trip and a drained connection pool is precisely
/// how a sweep stops returning.
///
/// Honest arithmetic: one indexed query, plus at most RECLAIM_LIMIT give-ups,
/// each of which is a claim and a fail (two statements and two Redis round
/// trips) awaited in series. A minute is that with room, and nothing about a
/// healthy pass comes close to it — a healthy pass is milliseconds, because the
/// drives it starts are detached.
pub const RECLAIM_MAX_RUN_MS: u64 = 60_000;

/// HOW MANY RUNS ONE PASS MAY TOUCH.
///
/// The bound is not politeness to Postgres; the query is indexed and cheap. It
/// is politeness to THIS PROCESS. The moment with the most orphaned runs is the
/// moment after a deploy or a crash, which is the same moment an instance is
/// coldest — and every run this pass hands over starts a driver that will run
/// steps, call models and write. Unbounded, one sweep after a bad night could
/// start hundreds of concurrent drives on a freshly booted box and knock it
/// over, which would orphan them again: a recovery mechanism that causes the
/// outage it recovers from.
///
/// Twenty-five per pass on a 30s interval drains a backlog at 50 runs a minute,
/// in staleness order (`due` sorts by expiry, oldest first), so nothing starves
/// and the queue drains in the order it fell behind.
pub const RECLAIM_LIMIT: i64 = 25;

// ── Deps ─────────────────────────────────────────────────────────────────────

/// THE reclaim query edge. `store.due` already means "runs nobody is driving":
/// in ('queued','running') with a lease that is null or expired.
pub type DueFn =
    Arc<dyn Fn(i64) -> BoxFuture<'static, Result<Vec<RunRow>, sqlx::Error>> + Send + Sync>;

/// The hand-over edge. The error is a String because that is its whole payload
/// here: the sweep counts it and the log/job line carries its text, exactly as
/// TS carries `errText(e)` — nothing downstream branches on the error's type.
pub type DriveFn =
    Arc<dyn Fn(String) -> BoxFuture<'static, Result<DriveResult, String>> + Send + Sync>;

/// Re-enter the run. The driver takes the lease, bumps `attempt`, re-enters
/// `step()` with the last persisted checkpoint, and owns every write and every
/// publish that follows.
pub struct ReclaimDeps {
    pub due: DueFn,
    pub definition_for: DefinitionForFn,
    pub drive: DriveFn,
    pub now: NowFn,
}

/// The real due edge over any `RunStore`. The Arc is cloned into the future so
/// the borrow the trait wants never has to outlive the closure.
pub fn due_fn(store: Arc<dyn RunStore>) -> DueFn {
    Arc::new(move |limit| {
        let store = store.clone();
        Box::pin(async move { store.due(limit).await })
    })
}

/// The real drive edge over `run::drive`. Exists so the assembly slice wires
/// one function instead of restating the error mapping.
pub fn drive_fn(deps: Arc<RunDeps>) -> DriveFn {
    Arc::new(move |run_id| {
        let deps = deps.clone();
        Box::pin(async move { drive(&run_id, &deps).await.map_err(|e| e.to_string()) })
    })
}

// ── What a pass did ──────────────────────────────────────────────────────────

/// Every field a u32 count over the rows the pass scanned; `stalest_ms` is a
/// duration. `Copy` on purpose: one pass is one small value a caller can hold,
/// log, and compare against the last one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReclaimSweep {
    /// Rows the query returned. Bounded by `limit` — see RECLAIM_LIMIT.
    pub scanned: u32,
    /// Runs handed to a driver. Counts HAND-OVERS, not successful claims: a run
    /// another instance is already driving comes back `busy` after one Redis
    /// round trip, and that is a normal, cheap outcome rather than a failure.
    pub driven: u32,
    /// Of `driven`, the ones that were `running` with an expired lease — i.e.
    /// the TRUE reclaims, the ones whose driver died. The rest were `queued`
    /// runs nobody had picked up. Worth its own number: a fleet with a steady
    /// stream of these is a fleet whose processes keep dying.
    pub reclaimed: u32,
    /// Runs a driver gave up on this pass: they had spent their attempts, so
    /// the driver filed them as errors instead of re-entering them.
    pub given_up: u32,
    /// Due, but no definition for the kind on THIS instance — a row from a
    /// newer deploy, or a module not in this process's graph. Left alone for
    /// an instance that has it; not an error and never a reason to fail the
    /// run.
    pub unknown_kinds: u32,
    /// Rows the query returned that a driver still holds. Should be zero — the
    /// query already excludes them — so a non-zero count is a real signal.
    pub live: u32,
    /// Rows the query returned in a state no driver advances. `awaiting` is
    /// the one that matters: see the guard in the pass. Should also be zero.
    pub not_drivable: u32,
    /// Awaited hand-overs that THREW. Not "the run failed" — the driver files
    /// that on the row — but "the runtime under the sweeper is broken".
    pub failed: u32,
    /// How long the stalest lease in this pass had been expired. The
    /// queue-depth number: rising means the sweeper is not keeping up with the
    /// bound.
    pub stalest_ms: i64,
}

// ── The pass ─────────────────────────────────────────────────────────────────

/// Find the runs nobody is driving and put them back to work.
///
/// Safe to call from anywhere, on any instance, as often as you like. Every run
/// it touches takes its own Redis lease, so a run somebody else is already
/// driving costs one refused SET NX and nothing else.
pub async fn sweep_reclaimable_runs(
    limit: Option<i64>,
    deps: &ReclaimDeps,
) -> Result<ReclaimSweep, sqlx::Error> {
    let limit = limit.unwrap_or(RECLAIM_LIMIT).max(1);

    // NOT wrapped in a catch. A query that cannot run means the sweeper cannot
    // see the queue at all, and the honest report of that is an error the
    // scheduler records as a failure and the unhealthy-jobs check puts on
    // /observability. Catching it here would turn "durability is not happening"
    // into a quiet pass that reports zero runs due, which reads identically to
    // a healthy idle fleet.
    let due = (deps.due)(limit).await?;
    let mut out = ReclaimSweep {
        scanned: due.len() as u32,
        ..ReclaimSweep::default()
    };
    if due.is_empty() {
        return Ok(out);
    }

    let now = (deps.now)();

    for run in due {
        // ── The guard this file exists for ────────────────────────────────
        // `awaiting` is NOT stale. It is parked on a person, which is a
        // healthy state that may last days, and driving it would re-ask the
        // question it is already parked on — or, if this sweeper had been
        // written the way research.ts writes one, mark it failed for the crime
        // of waiting. `is_drivable` is the shared predicate (define.rs) rather
        // than a state comparison spelled out again here, and the store's
        // query already agrees with it. This says it a SECOND time on purpose:
        // the sweep is the one place in the system that can wake a run from
        // the outside, and a rule that lives only in a WHERE clause is one
        // index rewrite away from not being there at all.
        if !is_drivable(run.state) {
            out.not_drivable += 1;
            tracing::warn!(
                "{LOG} {} ({}) came back from the due query in state \"{}\" — leaving it alone. {}",
                run.id,
                run.kind,
                state_name(run.state),
                if run.state == RunState::Awaiting {
                    "A parked run is waiting for a person, not stuck, and this sweeper never fails \
                     one however long it sits."
                } else {
                    "A finished run is nobody's to drive."
                }
            );
            continue;
        }

        // A lease that has not expired means a driver is alive and stepping
        // this run right now. The query already excludes these; saying it
        // again costs an integer comparison and protects the one case where
        // being wrong is expensive — two drivers in one run is how a side
        // effect happens twice. Read from the app clock rather than the
        // database's, so the failure mode under clock skew is a run reclaimed
        // one pass late (harmless: the next pass takes it) rather than one
        // reclaimed early (which the Redis lease would refuse anyway).
        //
        // An UNPARSEABLE expiry counts as expired, same as TS: a lease stamp
        // the app cannot read is not evidence a driver is alive.
        let expires_at = run
            .lease_expires_at
            .as_deref()
            .and_then(crate::agent_auth::iso_to_epoch_ms);
        if expires_at.is_some_and(|at| at > now) {
            out.live += 1;
            continue;
        }

        let Some(def) = (deps.definition_for)(&run.kind) else {
            // NOT an error on the row, for the same reason `drive` refuses to
            // make it one: a kind this instance never imported is still
            // perfectly drivable by an instance that has it, and failing the
            // run here would destroy work on the strength of a local import
            // graph.
            out.unknown_kinds += 1;
            tracing::warn!(
                "{LOG} {}: no definition for kind \"{}\" on this instance — leaving it for one that \
                 has it",
                run.id,
                run.kind
            );
            continue;
        };

        let stale_ms = expires_at.map_or(0, |at| (now - at).max(0));
        if stale_ms > out.stalest_ms {
            out.stalest_ms = stale_ms;
        }

        // ── Hand it over ──────────────────────────────────────────────────
        //
        // WILL THE DRIVER GIVE UP ON THIS ONE? Asked here for ONE reason:
        // whether to await the hand-over or detach it. It is a PREDICTION, not
        // a second decision — the give-up itself is `drive`'s, made after its
        // claim, and it is the only code that writes the error and publishes
        // it. Getting the prediction wrong costs a wait that was not needed or
        // a report line that is not there; it cannot produce a wrong outcome
        // for the run.
        //
        // The arithmetic mirrors store.claim's, deliberately, because that is
        // what it is predicting: the claim adds one to `attempt` only when the
        // previous state was `running` (a reclaim), so this is the count the
        // driver will be holding when it makes the call. The TS `??
        // DEFAULT_MAX_ATTEMPTS` collapsed when `max_attempts` became a
        // required field on the definition.
        let max_attempts = def.max_attempts;
        let attempt_after_claim = run.attempt + i32::from(run.state == RunState::Running);
        let spent = attempt_after_claim >= max_attempts as i32;

        if !spent {
            out.driven += 1;
            if run.state == RunState::Running {
                out.reclaimed += 1;
                tracing::warn!(
                    "{LOG} resuming {} ({}) — its driver stopped renewing {} ago at phase \"{}\", \
                     attempt {} of {}. It re-enters from the last persisted checkpoint.",
                    run.id,
                    run.kind,
                    dur(stale_ms),
                    run.phase,
                    attempt_after_claim,
                    max_attempts
                );
            }
            // DETACHED, and the sweep resolves as soon as the drives are
            // started. Awaiting them would make one pass a single-file queue
            // whose tick lasts as long as the slowest run in the workspace —
            // and the scheduler's overlap guard would then turn away every
            // tick behind it, so one long run would stop the whole reclaim
            // schedule. The cost of detaching is that a drive which errors
            // lands in the log and not in the scheduler's error state; `drive`
            // is total for everything except an unreachable store, so that
            // line is the alarm for exactly that case.
            let drive = deps.drive.clone();
            let (run_id, kind) = (run.id.clone(), run.kind.clone());
            tokio::spawn(async move {
                if let Err(e) = drive(run_id.clone()).await {
                    tracing::error!("{LOG} reclaim drive of {run_id} ({kind}) threw: {e}");
                }
            });
            continue;
        }

        // AWAITED, because this one is not going to do any work: the driver
        // will claim it, see that its attempts are spent, file the error and
        // return. That is two round trips, so the sweep can afford to watch —
        // and watching is what lets a pass REPORT the give-up. "3 runs were
        // abandoned this pass" is the sentence an operator needs; it is not
        // something they should have to reconstruct from scattered driver
        // logs.
        //
        // In series rather than in parallel: the bound is small, each is a
        // couple of statements, and a pass that opened twenty-five connections
        // at once to write twenty-five error rows would be picking a fight
        // with the pool at the exact moment (post-deploy) the pool is busiest.
        match (deps.drive)(run.id.clone()).await {
            Ok(res) if res.stop == DriveStop::Exhausted => {
                out.given_up += 1;
                // The row now carries the driver's message, which names the
                // attempt count and what each of those attempts did (stopped
                // without finishing or checkpointing). The log line adds what
                // only the SWEEP knew: where it died, how long ago, and whose
                // it was. Between them a person has the whole story without
                // opening a database.
                tracing::error!(
                    "{}",
                    give_up_line(&run, attempt_after_claim, res.error.as_deref(), now)
                );
            }
            Ok(_) => {
                // The prediction was wrong, or the row moved under us —
                // another instance claimed it first (`busy`), somebody
                // cancelled it, Redis was unreachable (`blocked`, and the row
                // is deliberately left exactly as it was). All normal; count
                // it as a hand-over and let the driver's own log speak.
                out.driven += 1;
                if run.state == RunState::Running {
                    out.reclaimed += 1;
                }
            }
            Err(e) => {
                out.failed += 1;
                tracing::error!("{LOG} hand-over of {} ({}) threw: {e}", run.id, run.kind);
            }
        }
    }

    Ok(out)
}

/// The give-up log line, split out so it is a pure function of what the pass
/// knew — same reason `describe_sweep` is a function: the sentence is the only
/// view most people will ever have of a run being abandoned, which makes it
/// worth a test rather than a format! buried in a match arm.
fn give_up_line(run: &RunRow, attempt_after_claim: i32, error: Option<&str>, now: i64) -> String {
    let mut line = format!(
        "{LOG} gave up on {} ({}) after {attempt_after_claim} attempt(s): {} — last progress \"{}\"",
        run.id,
        run.kind,
        error.unwrap_or("no message"),
        run.phase
    );
    // An unparseable `updated_at` drops the clause rather than the line — the
    // TS version guards `Number.isFinite` for the same reason.
    if let Some(moved) = crate::agent_auth::iso_to_epoch_ms(&run.updated_at) {
        line.push_str(&format!(" {} ago", dur((now - moved).max(0))));
    }
    if let Some(err) = &run.error {
        line.push_str(&format!("; last error: {err}"));
    }
    match &run.owner_user_id {
        Some(owner) => line.push_str(&format!(". Its owner ({owner}) was waiting on it.")),
        None => line.push('.'),
    }
    line
}

// ── The job ──────────────────────────────────────────────────────────────────

/// What one pass reads as in the scheduler log and in the job's last result.
/// Separated from the job so it is a pure function of the numbers — the log
/// line is the only view most people will ever have of this job working, which
/// makes it worth a test rather than a template buried in a callback.
pub fn describe_sweep(r: &ReclaimSweep) -> String {
    let mut parts = vec![format!("{} handed to a driver", r.driven)];
    if r.reclaimed > 0 {
        parts.push(format!("{} reclaimed from a driver that died", r.reclaimed));
    }
    if r.given_up > 0 {
        parts.push(format!("{} given up on (attempts spent)", r.given_up));
    }
    if r.unknown_kinds > 0 {
        parts.push(format!(
            "{} of a kind this instance cannot drive",
            r.unknown_kinds
        ));
    }
    if r.live > 0 {
        parts.push(format!("{} still leased", r.live));
    }
    if r.not_drivable > 0 {
        parts.push(format!(
            "{} not drivable (parked or finished)",
            r.not_drivable
        ));
    }
    if r.stalest_ms > 0 {
        parts.push(format!("stalest lease expired {} ago", dur(r.stalest_ms)));
    }
    format!("{} run(s) due — {}", r.scanned, parts.join(", "))
}

/// One tick, as the scheduler contract wants it: a sentence for the log, None
/// for "nothing to do", and an Err for failure.
///
/// The error is a String because that is its whole payload at this boundary —
/// the scheduler records the text, exactly as TS records `errText`. Two
/// failure paths land here: a due query that could not run (mapped from the
/// store's error) and a pass with thrown hand-overs (composed below).
pub async fn run_reclaim_job(deps: &ReclaimDeps) -> Result<Option<String>, String> {
    let r = sweep_reclaimable_runs(None, deps)
        .await
        .map_err(|e| e.to_string())?;
    if r.scanned == 0 {
        return Ok(None);
    }
    let line = describe_sweep(&r);

    // A HAND-OVER THAT THREW IS A FAILED PASS, not a footnote. `drive` is total
    // for everything a run can do to itself; an error out of it means the store
    // or the lease is broken underneath the whole runtime, and the one place
    // that fact can reach a person is the scheduler's error state. The line
    // goes INTO the message rather than being lost with it — a failure report
    // that also says what the pass managed to do is a report somebody can act
    // on.
    if r.failed > 0 {
        return Err(format!(
            "{} run hand-over(s) threw — the runs store or lease is failing. {line}",
            r.failed
        ));
    }

    // A pass whose whole scan was runs other drivers already hold is a healthy
    // pass, and it says so rather than claiming credit for them.
    Ok(Some(line))
}

/// The wire name of a state, for log lines that quote the row. `RunState`'s
/// serde form is lowercase; `Debug` would say "Awaiting" where TS says
/// "awaiting", and the sentence is pinned by a test.
fn state_name(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::Running => "running",
        RunState::Awaiting => "awaiting",
        RunState::Done => "done",
        RunState::Error => "error",
        RunState::Cancelled => "cancelled",
    }
}

// ── The registration ────────────────────────────────────────────────────────

/// The job the scheduler runs, from a built deps bag — the four declared
/// timings in their four slots, and nothing invented here. Split out from the
/// registration so a test can read the numbers without a live registry.
pub fn reclaim_job_spec(deps: Arc<ReclaimDeps>) -> crate::scheduler::JobSpec {
    crate::scheduler::JobSpec {
        name: crate::scheduler::JobName::RunReclaim,
        every_ms: RECLAIM_EVERY_MS,
        first_run_delay_ms: Some(RECLAIM_FIRST_RUN_DELAY_MS),
        max_run_ms: Some(RECLAIM_MAX_RUN_MS),
        // NOT `per_instance`, and the JobSpec doc comment is why: this job's
        // input is the `runs` TABLE, which every instance can reach, and
        // `per_instance` is only for a job whose entire input lives inside one
        // process. So it takes the scheduler's lease, and the fleet does one
        // sweep per interval instead of one per instance per interval.
        //
        // Duplication would in fact be survivable here — every run takes its
        // own Redis lease, so a second sweeper's hand-overs would come back
        // `busy` — but "survivable" is not the bar for a job that starts
        // drives which bill model calls. The trade the lease buys, worth
        // naming: the instance that wins the tick drives everything it
        // reclaims, so recovery load lands on one box. Different instances
        // win different intervals, so it evens out; and if reclaim throughput
        // ever becomes the bottleneck the answer is a larger RECLAIM_LIMIT,
        // not N instances scanning the same index.
        per_instance: false,
        run: Arc::new(move || {
            let deps = deps.clone();
            Box::pin(async move { run_reclaim_job(&deps).await })
        }),
    }
}

/// Declare the sweep to the scheduler. TS registers at module load next to the
/// work; Rust's deps are runtime values (the pool, the realtime fan-out), so
/// the registration is a function the flip calls from boot — same rule, same
/// consequence: the call is what puts the job in the runtime graph, and
/// 'run-reclaim' is in REQUIRED_JOBS, so an instance that somehow boots
/// without reaching it prints a MISSING JOBS error instead of running with no
/// durability at all.
pub fn register_reclaim_job(deps: Arc<ReclaimDeps>) {
    crate::scheduler::register_job(reclaim_job_spec(deps));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // A row as the due query would hand it back: the ordinary reclaim case —
    // `running`, lease expired a minute ago, first entry. The give-up line is
    // pinned here because it is a pure function; the pass itself is exercised
    // end-to-end in tests/runs_reclaim.rs.
    fn row() -> RunRow {
        let now = 1_700_000_000_000_i64;
        RunRow {
            id: "run-1".into(),
            kind: "test-kind".into(),
            owner_user_id: Some("user-1".into()),
            subject_type: None,
            subject_id: None,
            state: RunState::Running,
            phase: "reading the sources".into(),
            checkpoint: json!({ "page": 3 }),
            input: serde_json::Value::Null,
            result: serde_json::Value::Null,
            error: None,
            attempt: 0,
            lease_owner: Some("dead-driver-token".into()),
            lease_expires_at: Some(crate::agent_auth::epoch_ms_to_iso(now - 60_000)),
            approval_key: None,
            decision: None,
            created_at: crate::agent_auth::epoch_ms_to_iso(now - 600_000),
            updated_at: crate::agent_auth::epoch_ms_to_iso(now - 120_000),
            started_at: Some(crate::agent_auth::epoch_ms_to_iso(now - 600_000)),
            finished_at: None,
        }
    }

    #[test]
    fn give_up_line_names_the_count_the_diagnosis_and_the_phase_and_never_says_stale() {
        let line = give_up_line(
            &row(),
            3,
            Some(
                "run gave up after 3 attempt(s): each driver that took it stopped without finishing \
                 or checkpointing",
            ),
            1_700_000_000_000,
        );
        assert!(line.contains("gave up on run-1 (test-kind)"));
        assert!(line.contains("after 3 attempt(s)"));
        assert!(line.contains("stopped without finishing or checkpointing"));
        assert!(line.contains("last progress \"reading the sources\""));
        // The sentence research.ts shipped, which this file exists to delete.
        assert!(!line.contains("went stale"));
        // How long ago it last moved, and whose it was.
        assert!(line.contains("2 minutes ago"));
        assert!(line.contains(". Its owner (user-1) was waiting on it."));
    }

    #[test]
    fn give_up_line_says_no_message_and_drops_whose_it_is_without_an_owner() {
        let mut r = row();
        r.owner_user_id = None;
        let line = give_up_line(&r, 1, None, 1_700_000_000_000);
        assert!(line.contains("attempt(s): no message"));
        assert!(line.ends_with('.'));
        assert!(!line.contains("was waiting on it"));
    }

    /// The four declared timings, provably the four numbers the job runs with
    /// — the Rust half of runs/boot.test.ts's pin. A spec built with the
    /// constants in the wrong slot is a job that runs at the wrong rate while
    /// every comment still says the right thing.
    #[test]
    fn the_job_spec_carries_the_declared_timings() {
        let deps = Arc::new(ReclaimDeps {
            due: Arc::new(|_| Box::pin(async { Ok(Vec::new()) })),
            definition_for: Arc::new(|_| None),
            drive: Arc::new(|_| {
                Box::pin(async { unreachable!("the spec test never runs the job") })
            }),
            now: Arc::new(|| 0),
        });
        let spec = reclaim_job_spec(deps);
        assert_eq!(spec.name.as_str(), "run-reclaim");
        assert_eq!(spec.every_ms, 30_000);
        assert_eq!(spec.first_run_delay_ms, Some(20_000));
        assert_eq!(spec.max_run_ms, Some(60_000));
        assert!(
            !spec.per_instance,
            "the runs table is fleet-shared; it takes the lease"
        );
    }
}
