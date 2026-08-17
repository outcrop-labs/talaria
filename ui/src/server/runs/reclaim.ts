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
// `isDrivable` (server/runs/define.ts, which says the same thing in its doc
// comment) and there is a test pinning it.
//
// WHO WRITES WHAT — the split with run.ts, stated once so nobody has to infer
// it. `drive()` is the ONLY code in the runs system that writes a state
// transition or publishes one; every write in store.ts is a compare-and-set on
// the lease token, and the driver holds that token. So this file writes
// NOTHING. It selects, it bounds, it hands over, and it reports. In particular
// it does not publish an event of its own: the transitions this sweep causes
// are published by the driver that performs them (a checkpoint, a park, a
// give-up), and an event from here would either duplicate one of those or —
// worse — announce a state the row is not in yet, because a sweep cannot know
// whether its kick will win the run's Redis lease.
//
// TESTABILITY IS A DESIGN CONSTRAINT (see harness/run.ts, and runs/run.ts next
// door). Every edge — the query, the registry, the driver, the clock — is a
// field on `ReclaimDeps` defaulted to the real thing, so reclaim.test.ts drives
// the whole sweeper with no database, no Redis and no clock.
import { registerJob, type JobSpec, type JobName } from '../scheduler'
import { DEFAULT_MAX_ATTEMPTS, isDrivable, runDefinition, type AnyRunDefinition, type RunRow } from './define'
import { drive, type DriveResult } from './run'
import { pgRunStore } from './store'

const LOG = '[runs/reclaim]'

const errText = (e: unknown) => (e instanceof Error ? (e.stack ?? e.message) : String(e))

/** Operator-facing durations. Say seconds when it is seconds: "0 minutes" for a
 *  twenty-second-old lease is the kind of rounding that makes somebody stop
 *  reading the line. */
const dur = (ms: number): string => (ms < 90_000 ? `${Math.max(0, Math.round(ms / 1_000))}s` : `${Math.round(ms / 60_000)} minutes`)

// ── The declared timings ─────────────────────────────────────────────────────
//
// Exported as constants rather than buried in the spec literal because two of
// them are promises the rest of the system reads: LIMIT is the recovery RATE
// (with EVERY_MS), and MAX_RUN_MS is what `unhealthyJobs()` uses to call a
// wedged sweep hung rather than slow. A reviewer should be able to see all four
// numbers and the reasoning for each in one place.

export const RECLAIM_JOB: JobName = 'run-reclaim'

/** HOW FAST A CRASHED RUN COMES BACK. A run's lease is its definition's
 *  `maxStepMs`, so the row becomes reclaimable roughly one step after the
 *  process holding it died; this interval is the rest of the delay a waiting
 *  person sees. Thirty seconds makes the worst case "one step, plus half a
 *  minute" — fast enough that a deploy reads as a pause rather than a stall,
 *  and cheap enough to be free: one pass is a single partial-index scan
 *  (runs_reclaim_idx), and the scheduler's lease means it happens once per
 *  interval across the whole fleet, not once per instance. */
export const RECLAIM_EVERY_MS = 30_000

/** Let the instance settle before it starts re-entering steps.
 *
 *  Shorter than comms-decay's two minutes because what this job resumes is work
 *  a person is actively watching, and longer than zero for the reason every
 *  `firstRunDelayMs` in this codebase exists: a crash-looping instance must
 *  never reach a job that writes. And this one writes through other people's
 *  side effects — a reclaimed step can bill a model call, open a PR or send a
 *  DM, because the runtime is AT-LEAST-ONCE. A boot loop that swept
 *  immediately would re-enter the same step on every restart. */
export const RECLAIM_FIRST_RUN_DELAY_MS = 20_000

/** THE OUTSIDE BOUND FOR ONE PASS, and it is read: `unhealthyJobs()` calls a run
 *  that outlives it HUNG rather than slow, which is the only way a sweeper that
 *  stopped coming back becomes visible to anybody. That is the exact failure the
 *  scheduler header warns about — a job that neither throws nor returns leaves
 *  `failures` at 0, `runs` at 0 and `running` true forever — and it is a real
 *  risk here, because every await in a pass is a database round trip and a
 *  drained connection pool is precisely how a sweep stops returning.
 *
 *  Honest arithmetic: one indexed query, plus at most RECLAIM_LIMIT give-ups,
 *  each of which is a claim and a fail (two statements and two Redis round
 *  trips) awaited in series. A minute is that with room, and nothing about a
 *  healthy pass comes close to it — a healthy pass is milliseconds, because the
 *  drives it starts are detached. */
export const RECLAIM_MAX_RUN_MS = 60_000

/** HOW MANY RUNS ONE PASS MAY TOUCH.
 *
 *  The bound is not politeness to Postgres; the query is indexed and cheap. It
 *  is politeness to THIS PROCESS. The moment with the most orphaned runs is the
 *  moment after a deploy or a crash, which is the same moment an instance is
 *  coldest — and every run this pass hands over starts a driver that will run
 *  steps, call models and write. Unbounded, one sweep after a bad night could
 *  start hundreds of concurrent drives on a freshly booted box and knock it
 *  over, which would orphan them again: a recovery mechanism that causes the
 *  outage it recovers from.
 *
 *  Twenty-five per pass on a 30s interval drains a backlog at 50 runs a minute,
 *  in staleness order (`due` sorts by expiry, oldest first), so nothing starves
 *  and the queue drains in the order it fell behind. */
export const RECLAIM_LIMIT = 25

// ── Deps ─────────────────────────────────────────────────────────────────────

export interface ReclaimDeps {
  /** THE reclaim query. `store.due` already means "runs nobody is driving": in
   *  ('queued','running') with a lease that is null or expired. */
  due: (args: { limit: number }) => Promise<RunRow[]>
  definitionFor: (kind: string) => AnyRunDefinition | null
  /** Re-enter the run. The driver takes the lease, bumps `attempt`, re-enters
   *  `step()` with the last persisted checkpoint, and owns every write and
   *  every publish that follows. */
  drive: (runId: string) => Promise<DriveResult>
  now: () => number
}

const REAL_DEPS: ReclaimDeps = {
  due: (args) => pgRunStore.due(args),
  definitionFor: runDefinition,
  drive: (runId) => drive(runId),
  now: () => Date.now(),
}

// ── What a pass did ──────────────────────────────────────────────────────────

export interface ReclaimSweep {
  /** Rows the query returned. Bounded by `limit` — see RECLAIM_LIMIT. */
  scanned: number
  /** Runs handed to a driver. Counts HANDOVERS, not successful claims: a run
   *  another instance is already driving comes back `busy` after one Redis
   *  round trip, and that is a normal, cheap outcome rather than a failure. */
  driven: number
  /** Of `driven`, the ones that were `running` with an expired lease — i.e. the
   *  TRUE reclaims, the ones whose driver died. The rest were `queued` runs
   *  nobody had picked up. Worth its own number: a fleet with a steady stream
   *  of these is a fleet whose processes keep dying. */
  reclaimed: number
  /** Runs a driver gave up on this pass: they had spent their attempts, so the
   *  driver filed them as errors instead of re-entering them. */
  givenUp: number
  /** Due, but no definition for the kind on THIS instance — a row from a newer
   *  deploy, or a module not in this process's graph. Left alone for an
   *  instance that has it; not an error and never a reason to fail the run. */
  unknownKinds: number
  /** Rows the query returned that a driver still holds. Should be zero — the
   *  query already excludes them — so a non-zero count is a real signal. */
  live: number
  /** Rows the query returned in a state no driver advances. `awaiting` is the
   *  one that matters: see the guard below. Should also be zero. */
  notDrivable: number
  /** Awaited hand-overs that THREW. Not "the run failed" — the driver files
   *  that on the row — but "the runtime under the sweeper is broken". */
  failed: number
  /** How long the stalest lease in this pass had been expired. The queue-depth
   *  number: rising means the sweeper is not keeping up with the bound. */
  stalestMs: number
}

const emptySweep = (): ReclaimSweep => ({
  scanned: 0,
  driven: 0,
  reclaimed: 0,
  givenUp: 0,
  unknownKinds: 0,
  live: 0,
  notDrivable: 0,
  failed: 0,
  stalestMs: 0,
})

// ── The pass ─────────────────────────────────────────────────────────────────

/** Find the runs nobody is driving and put them back to work.
 *
 *  Safe to call from anywhere, on any instance, as often as you like. Every run
 *  it touches takes its own Redis lease, so a run somebody else is already
 *  driving costs one refused SET NX and nothing else. */
export async function sweepReclaimableRuns(opts: { limit?: number } = {}, deps: Partial<ReclaimDeps> = {}): Promise<ReclaimSweep> {
  const d: ReclaimDeps = { ...REAL_DEPS, ...deps }
  const limit = Math.max(1, Math.floor(opts.limit ?? RECLAIM_LIMIT))

  // NOT wrapped in a try. A query that cannot run means the sweeper cannot see
  // the queue at all, and the honest report of that is a throw the scheduler
  // records as a failure and `unhealthyJobs()` puts on /observability. Catching
  // it here would turn "durability is not happening" into a quiet pass that
  // reports zero runs due, which reads identically to a healthy idle fleet.
  const due = await d.due({ limit })
  const out = emptySweep()
  out.scanned = due.length
  if (!due.length) return out

  const now = d.now()

  for (const run of due) {
    // ── The guard this file exists for ────────────────────────────────────
    // `awaiting` is NOT stale. It is parked on a person, which is a healthy
    // state that may last days, and driving it would re-ask the question it is
    // already parked on — or, if this sweeper had been written the way
    // research.ts writes one, mark it failed for the crime of waiting.
    // `isDrivable` is the shared predicate (define.ts) rather than a state
    // comparison spelled out again here, and the store's query already agrees
    // with it. This says it a SECOND time on purpose: the sweep is the one
    // place in the system that can wake a run from the outside, and a rule that
    // lives only in a WHERE clause is one index rewrite away from not being
    // there at all.
    if (!isDrivable(run.state)) {
      out.notDrivable++
      console.warn(
        `${LOG} ${run.id} (${run.kind}) came back from the due query in state "${run.state}" — leaving it alone. ` +
          (run.state === 'awaiting'
            ? 'A parked run is waiting for a person, not stuck, and this sweeper never fails one however long it sits.'
            : 'A finished run is nobody\'s to drive.'),
      )
      continue
    }

    // A lease that has not expired means a driver is alive and stepping this
    // run right now. The query already excludes these; saying it again costs an
    // integer comparison and protects the one case where being wrong is
    // expensive — two drivers in one run is how a side effect happens twice.
    // Read from the app clock rather than the database's, so the failure mode
    // under clock skew is a run reclaimed one pass late (harmless: the next
    // pass takes it) rather than one reclaimed early (which the Redis lease
    // would refuse anyway).
    const expiresAt = run.leaseExpiresAt ? Date.parse(run.leaseExpiresAt) : null
    if (expiresAt !== null && Number.isFinite(expiresAt) && expiresAt > now) {
      out.live++
      continue
    }

    const def = d.definitionFor(run.kind)
    if (!def) {
      // NOT an error on the row, for the same reason `drive` refuses to make it
      // one: a kind this instance never imported is still perfectly drivable by
      // an instance that has it, and failing the run here would destroy work on
      // the strength of a local import graph.
      out.unknownKinds++
      console.warn(`${LOG} ${run.id}: no definition for kind "${run.kind}" on this instance — leaving it for one that has it`)
      continue
    }

    const staleMs = expiresAt !== null && Number.isFinite(expiresAt) ? Math.max(0, now - expiresAt) : 0
    if (staleMs > out.stalestMs) out.stalestMs = staleMs

    // ── Hand it over ──────────────────────────────────────────────────────
    //
    // WILL THE DRIVER GIVE UP ON THIS ONE? Asked here for ONE reason: whether
    // to await the hand-over or detach it. It is a PREDICTION, not a second
    // decision — the give-up itself is `drive`'s, made after its claim, and it
    // is the only code that writes the error and publishes it. Getting the
    // prediction wrong costs a wait that was not needed or a report line that
    // is not there; it cannot produce a wrong outcome for the run.
    //
    // The arithmetic mirrors store.claim's, deliberately, because that is what
    // it is predicting: the claim adds one to `attempt` only when the previous
    // state was `running` (a reclaim), so this is the count the driver will be
    // holding when it makes the call.
    const maxAttempts = def.maxAttempts ?? DEFAULT_MAX_ATTEMPTS
    const attemptAfterClaim = run.attempt + (run.state === 'running' ? 1 : 0)
    const spent = attemptAfterClaim >= maxAttempts

    if (!spent) {
      out.driven++
      if (run.state === 'running') {
        out.reclaimed++
        console.warn(
          `${LOG} resuming ${run.id} (${run.kind}) — its driver stopped renewing ${dur(staleMs)} ago at phase "${run.phase}", ` +
            `attempt ${attemptAfterClaim} of ${maxAttempts}. It re-enters from the last persisted checkpoint.`,
        )
      }
      // DETACHED, and the sweep resolves as soon as the drives are started.
      // Awaiting them would make one pass a single-file queue whose tick lasts
      // as long as the slowest run in the workspace — and the scheduler's
      // overlap guard would then turn away every tick behind it, so one long
      // run would stop the whole reclaim schedule. The cost of detaching is
      // that a drive which throws lands in the log and not in the scheduler's
      // error state; `drive` is total for everything except an unreachable
      // store, so that line is the alarm for exactly that case.
      void d.drive(run.id).catch((e) => console.error(`${LOG} reclaim drive of ${run.id} (${run.kind}) threw:`, errText(e)))
      continue
    }

    // AWAITED, because this one is not going to do any work: the driver will
    // claim it, see that its attempts are spent, file the error and return.
    // That is two round trips, so the sweep can afford to watch — and watching
    // is what lets a pass REPORT the give-up. "3 runs were abandoned this pass"
    // is the sentence an operator needs; it is not something they should have
    // to reconstruct from scattered driver logs.
    //
    // In series rather than in parallel: the bound is small, each is a couple
    // of statements, and a pass that opened twenty-five connections at once to
    // write twenty-five error rows would be picking a fight with the pool at
    // the exact moment (post-deploy) the pool is busiest.
    try {
      const res = await d.drive(run.id)
      if (res.stop === 'exhausted') {
        out.givenUp++
        // The row now carries the driver's message, which names the attempt
        // count and what each of those attempts did (stopped without finishing
        // or checkpointing). The log line adds what only the SWEEP knew: where
        // it died, how long ago, and whose it was. Between them a person has
        // the whole story without opening a database.
        const lastMoved = Date.parse(run.updatedAt)
        console.error(
          `${LOG} gave up on ${run.id} (${run.kind}) after ${attemptAfterClaim} attempt(s): ${res.error ?? 'no message'} — ` +
            `last progress "${run.phase}"` +
            (Number.isFinite(lastMoved) ? ` ${dur(Math.max(0, now - lastMoved))} ago` : '') +
            (run.error ? `; last error: ${run.error}` : '') +
            (run.ownerUserId ? `. Its owner (${run.ownerUserId}) was waiting on it.` : '.'),
        )
        continue
      }
      // The prediction was wrong, or the row moved under us — another instance
      // claimed it first (`busy`), somebody cancelled it, Redis was unreachable
      // (`blocked`, and the row is deliberately left exactly as it was). All
      // normal; count it as a hand-over and let the driver's own log speak.
      out.driven++
      if (run.state === 'running') out.reclaimed++
    } catch (e) {
      out.failed++
      console.error(`${LOG} hand-over of ${run.id} (${run.kind}) threw:`, errText(e))
    }
  }

  return out
}

// ── The job ──────────────────────────────────────────────────────────────────

/** What one pass reads as in the scheduler log and in `JobStatus.lastResult`.
 *  Separated from the job so it is a pure function of the numbers — the log
 *  line is the only view most people will ever have of this job working, which
 *  makes it worth a test rather than a template literal buried in a callback. */
export function describeSweep(r: ReclaimSweep): string {
  const parts = [
    `${r.driven} handed to a driver`,
    r.reclaimed ? `${r.reclaimed} reclaimed from a driver that died` : null,
    r.givenUp ? `${r.givenUp} given up on (attempts spent)` : null,
    r.unknownKinds ? `${r.unknownKinds} of a kind this instance cannot drive` : null,
    r.live ? `${r.live} still leased` : null,
    r.notDrivable ? `${r.notDrivable} not drivable (parked or finished)` : null,
    r.stalestMs ? `stalest lease expired ${dur(r.stalestMs)} ago` : null,
  ].filter((p): p is string => p !== null)
  return `${r.scanned} run(s) due — ${parts.join(', ')}`
}

/** One tick, as the scheduler contract wants it: a sentence for the log, null
 *  for "nothing to do", and a THROW for failure.
 *
 *  Split out of the spec literal so it takes deps like everything else in this
 *  tree — `JobSpec.run` takes no arguments, and a job whose body could only be
 *  tested by mocking a module would be the one piece of this system that is not
 *  driven by injection. */
export async function runReclaimJob(deps: Partial<ReclaimDeps> = {}): Promise<string | null> {
  const r = await sweepReclaimableRuns({}, deps)
  if (!r.scanned) return null
  const line = describeSweep(r)

  // A HAND-OVER THAT THREW IS A FAILED PASS, not a footnote. `drive` is total
  // for everything a run can do to itself; a throw out of it means the store or
  // the lease is broken underneath the whole runtime, and the one place that
  // fact can reach a person is the scheduler's error state. The line goes INTO
  // the message rather than being lost with it — a failure report that also
  // says what the pass managed to do is a report somebody can act on.
  if (r.failed) throw new Error(`${r.failed} run hand-over(s) threw — the runs store or lease is failing. ${line}`)

  // A pass whose whole scan was runs other drivers already hold is a healthy
  // pass, and it says so rather than claiming credit for them.
  return line
}

/** Exported so a test can read the declared timings without starting a
 *  scheduler, and so the four numbers above are provably the four numbers the
 *  job actually runs with. */
export const RECLAIM_JOB_SPEC: JobSpec = {
  name: RECLAIM_JOB,
  everyMs: RECLAIM_EVERY_MS,
  firstRunDelayMs: RECLAIM_FIRST_RUN_DELAY_MS,
  maxRunMs: RECLAIM_MAX_RUN_MS,
  // NOT `perInstance`, and the JobSpec doc comment is why: this job's input is
  // the `runs` TABLE, which every instance can reach, and `perInstance` is only
  // for a job whose entire input lives inside one process. So it takes the
  // scheduler's lease, and the fleet does one sweep per interval instead of one
  // per instance per interval.
  //
  // Duplication would in fact be survivable here — every run takes its own
  // Redis lease, so a second sweeper's hand-overs would come back `busy` — but
  // "survivable" is not the bar for a job that starts drives which bill model
  // calls. The trade the lease buys, worth naming: the instance that wins the
  // tick drives everything it reclaims, so recovery load lands on one box.
  // Different instances win different intervals, so it evens out; and if
  // reclaim throughput ever becomes the bottleneck the answer is a larger
  // RECLAIM_LIMIT, not N instances scanning the same index.
  run: () => runReclaimJob(),
}

// Registered at module load, next to the work — same rule as every other job in
// this tree, and the import is what puts it in the runtime graph.
//
// WIRING, DONE: server/runs/boot.ts imports this module and every run
// definition, and `src/routes/api/runs.$id.events.ts` imports boot.ts — the same
// route-graph path comms-decay, the digest and the notification mailer take.
// 'run-reclaim' is in REQUIRED_JOBS in server/scheduler.ts, so an instance that
// somehow boots without reaching this file prints a MISSING JOBS error instead
// of running with no durability at all. runs/boot.test.ts pins both halves.
registerJob(RECLAIM_JOB_SPEC)
