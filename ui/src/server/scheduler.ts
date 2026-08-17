// The background scheduler — the one place periodic server work is timed.
//
// WHY THIS FILE EXISTS
//   Talaria's three periodic jobs used to be "throttled kicks": comms decay,
//   the outreach sweep and the price refresh were each fired opportunistically
//   from a REQUEST handler, guarded by a module-level timestamp so they ran at
//   most once an hour. That is not a schedule, it is a side effect of traffic.
//   An instance serving no requests ran NONE of them — comms never decayed,
//   outreach never swept, prices never refreshed — and the quieter the
//   deployment, the less of its own maintenance it did. A daily digest built on
//   that pattern would be missing on exactly the days it matters most.
//
//   So: jobs declare a name and an interval, and this module owns the timing.
//   Nothing in a request path decides when background work happens.
//
// WHAT THIS FILE GUARANTEES
//   1. It runs with zero traffic. `startScheduler()` is called once from
//      server-entry.js before listen(); the timers are the only trigger.
//   2. A throwing job does not kill the process, does not stop its own
//      schedule, and does NOT fail silently. Every failure is logged with the
//      job name, the elapsed time and the error, and is kept in `status()`.
//   3. A job never overlaps itself — not in this process (the `running` flag)
//      and not across processes (the Redis lease below).
//   4. Two instances do not double-run a job. See MULTI-INSTANCE.
//   5. Nothing here can produce an unhandled rejection. Timer callbacks are
//      synchronous and hand the async work to `attempt()`, which is total —
//      it catches everything, including a job that throws synchronously. The
//      `unhandledRejection` handler in server-entry.js is a backstop for bugs
//      elsewhere, not the mechanism that makes this safe.
//   6. SIGTERM clears the timers, so a redeploy stops starting new runs
//      immediately and waits (briefly) for whatever is in flight.
//
// MULTI-INSTANCE — the deliberate choice
//   Existing job claiming in this codebase is in-process only (`liveSessions`,
//   `continuing` are plain Sets), so a second instance duplicates work. For
//   these jobs duplication is not a wasted CPU cycle, it is user-visible harm:
//   comms decay ARCHIVES people's conversations and the outreach sweep SENDS
//   MESSAGES. Two instances would archive the same chat twice and DM the same
//   person twice.
//
//   Redis is already a hard dependency (sessions), so the scheduler leases each
//   job in Redis. The lease is not just a mutex — a mutex only stops the two
//   instances running the job at the SAME time, and would still let instance B
//   run comms decay a minute after instance A finished, i.e. twice per
//   interval. So the key is held for the rest of the interval after a run
//   completes: acquire with SET NX PX (short TTL, renewed while the job runs),
//   then on completion demote it to a "this period is spent" marker that
//   expires just before the next tick is due. One run per interval, fleet-wide.
//
//   THE MECHANISM NOW LIVES IN server/runs/lease.ts and this file keeps the
//   POLICY. That split is deliberate and the paragraph above is why: the
//   runs runtime needs the same compare-and-set-on-a-token discipline, but it
//   needs the opposite policy — it leases ONE ROW for ONE STEP and releases it
//   immediately, so the next step is claimable by any instance. Sharing the
//   primitive and not the policy is what lets both be right. Everything below
//   that reads as scheduler behavior — the TTL from `maxRunMs`, the hold to the
//   end of the interval, the `perInstance` exemption, and skipping the tick when
//   Redis is unreachable — is still decided here.
//
//   If Redis is unreachable the tick is SKIPPED, not run unguarded. Fail
//   closed: a missed hour of archiving is recoverable, a duplicate proactive
//   DM to a customer is not. The skip is logged, and the next tick retries.
//
//   The one exception is `perInstance` (see JobSpec): a job whose entire input
//   is an in-memory queue in THIS process has nothing shared to duplicate, and
//   leasing it would leave every instance but the lease-winner's queue undrained.
//
// SEEING FAILURES
//   `status()` is not decoration. Guarantee 2 is only worth anything if someone
//   reads it, so `unhealthyJobs()` distils the same state into the sentences an
//   operator needs, and server/alerts.ts turns those into alerts on
//   /observability. A background job that has started failing is the exact
//   thing you cannot discover by using the product.
//
//   A THROWN error is the easy half. The half that cost M1 a cold-boot outage
//   is the job that neither throws nor returns: `failures` stays 0, `runs`
//   stays 0, `running` stays true forever, and the overlap guard dutifully
//   turns away every tick behind it. `unhealthyJobs()` names that case, and the
//   "armed but never started a run" case next to it, because between them they
//   are the whole of "this work is not happening and nothing said so".
//
// NOT IN DEV
//   `vite dev` does not use server-entry.js, so the scheduler does not run on a
//   developer's laptop. That is deliberate: nobody wants their dev process
//   archiving production chats or sending proactive DMs. Set
//   TALARIA_SCHEDULER=off to get the same silence in production — the kill
//   switch to deploy behind if a job ever misbehaves.
import { acquireLease, demoteLease, instanceId, keepLeaseAlive, leaseHolder, leaseKey, type LeaseHeartbeat, type LeaseToken } from './runs/lease'

/** Every job this deployment expects to be running. Adding a name here and
 *  nowhere else fails the boot check below, which is the point: a job whose
 *  module never got imported would otherwise be invisible — the exact failure
 *  mode (background work that silently does not happen) this file exists to
 *  end. */
export type JobName =
  | 'comms-decay'
  | 'outreach-sweep'
  | 'price-refresh'
  | 'daily-digest'
  | 'approval-escalation'
  | 'notification-mail'
  // server/runs/reclaim.ts — the sweep that re-enters a run whose driver died.
  | 'run-reclaim'

const REQUIRED_JOBS: JobName[] = [
  'comms-decay',
  'outreach-sweep',
  'price-refresh',
  // Both live in server/digest.ts. They are REQUIRED rather than optional for
  // the reason the list exists: their failure mode is silence. A digest that
  // never arrives and an approval that never escalates look exactly like a
  // quiet week, so the boot check has to be what notices the module fell out
  // of the graph.
  'daily-digest',
  'approval-escalation',
  // server/notifications.ts. Same reason again, and the sharpest case of it:
  // if this job is not running, notification email is queued and never sent,
  // and the ONLY symptom is mail that does not arrive.
  'notification-mail',
  // server/runs/reclaim.ts, reached through server/runs/boot.ts. The sharpest
  // case yet, and the reason this name was held back until a kind shipped: a
  // deployment missing this job serves every request correctly, starts every
  // research run, fitness sweep and work session, and writes every checkpoint —
  // and resumes none of them. The symptom is a long action that stops mid-way
  // after a deploy, which from the outside is indistinguishable from one that
  // is merely slow. Kinds ship now (research, fitness-sweep, rag-reindex,
  // rag-backfill, work-session), so the module has an importer and this is a
  // real alarm rather than a trained-out one.
  'run-reclaim',
]

export interface JobSpec {
  name: JobName
  /** How often to attempt the job. The scheduler owns this; callers do not. */
  everyMs: number
  /** Wait this long after start before the first attempt. Staggers boot, and
   *  means a crash-looping instance never reaches a job that writes. */
  firstRunDelayMs?: number
  /** How long one run is expected to take, at the OUTSIDE. Two things read it,
   *  so it is a real declaration and not a lease knob:
   *    · the Redis lease is held (and renewed) for this long, so a crashed
   *      instance's job becomes available to another instance after roughly
   *      this delay;
   *    · `unhealthyJobs()` calls a run that outlives it HUNG rather than slow.
   *      A `perInstance` job takes no lease and this is still the number that
   *      makes its hang visible, so set it honestly there too. */
  maxRunMs?: number
  /** Skip the Redis lease and run on EVERY instance, every interval.
   *
   *  The lease exists because the default job operates on SHARED state — the
   *  database — where a second instance doing the same pass archives the same
   *  chat twice or DMs the same person twice. A `perInstance` job operates on
   *  state that only this process can see (an in-memory queue), so the lease
   *  would not prevent duplication; it would prevent WORK. One instance would
   *  win the lease each interval and drain its own queue, and every other
   *  instance's queue would sit there untouched forever.
   *
   *  So this flag is narrow and it is load-bearing: set it only when the job's
   *  entire input lives inside this process. If a job reads or writes rows that
   *  another instance can also reach, it needs the lease. The in-process
   *  `running` flag is still the overlap guard either way. */
  perInstance?: boolean
  /** The work. Return a short human sentence for the log, or null for "nothing
   *  to do" (logged quietly). THROW to report failure — do not swallow. */
  run: () => Promise<string | null>
}

export interface JobStatus {
  name: JobName
  everyMs: number
  /** The outside bound for ONE run (JobSpec.maxRunMs, or the default). Past it
   *  a run is not slow, it is stuck — see unhealthyJobs. */
  maxRunMs: number
  running: boolean
  /** How long the CURRENT run has been going, or null when idle. The only
   *  field that can distinguish a wedged job from a healthy one. */
  runningForMs: number | null
  /** When the first run was due, or null when the scheduler never armed this
   *  job. Without it a job whose timer never fired has no "should have run by
   *  now" to be late against. */
  firstRunDueAt: string | null
  runs: number
  failures: number
  /** Ticks skipped because the previous run was still going (this process). */
  selfOverlaps: number
  /** Ticks skipped because another instance held the lease, or Redis was down. */
  leaseSkips: number
  lastStartedAt: string | null
  lastFinishedAt: string | null
  lastDurationMs: number | null
  lastResult: string | null
  lastError: string | null
}

interface JobState {
  spec: JobSpec
  timer: ReturnType<typeof setTimeout> | null
  interval: ReturnType<typeof setInterval> | null
  running: boolean
  inFlight: Promise<void> | null
  runs: number
  failures: number
  selfOverlaps: number
  leaseSkips: number
  startedAt: number | null
  finishedAt: number | null
  durationMs: number | null
  result: string | null
  error: string | null
  /** When startScheduler() armed this job, and when its first run was due. */
  armedAt: number | null
  firstRunDueAt: number | null
}

const DEFAULT_MAX_RUN_MS = 10 * 60_000
const LOG = '[scheduler]'

// Cached on globalThis so an HMR reload (or a stray double-import) reuses one
// registry instead of quietly running everything twice.
const g = globalThis as unknown as { __talariaJobs?: Map<JobName, JobState> }
const jobs: Map<JobName, JobState> = (g.__talariaJobs ??= new Map())

let started = false
let stopping = false

const errText = (e: unknown) => (e instanceof Error ? (e.stack ?? e.message) : String(e))
const errLine = (e: unknown) => (e instanceof Error ? e.message : String(e))

/** Declare a periodic job. Called at module load by the module that owns the
 *  work — the cadence lives next to the thing being scheduled, and the import
 *  is what puts the job in the runtime graph. */
export function registerJob(spec: JobSpec): void {
  const existing = jobs.get(spec.name)
  if (existing) {
    // Re-registration is a real bug (two modules claiming one name, or a
    // module evaluated twice), and a silently doubled job is one of the ways
    // "it sent the message twice" happens. Keep the first, say so loudly.
    console.error(`${LOG} job "${spec.name}" registered twice — keeping the first registration. This is a bug.`)
    return
  }
  jobs.set(spec.name, {
    spec,
    timer: null,
    interval: null,
    running: false,
    inFlight: null,
    runs: 0,
    failures: 0,
    selfOverlaps: 0,
    leaseSkips: 0,
    startedAt: null,
    finishedAt: null,
    durationMs: null,
    result: null,
    error: null,
    armedAt: null,
    firstRunDueAt: null,
  })
}

// ── The Redis lease: this file's half of it ──────────────────────────────────
//
// The mechanism (SET NX PX, the compare-and-set renewal, the token discipline)
// is server/runs/lease.ts. What is left here is everything that is a SCHEDULING
// decision, and none of it should move: which namespace, how long a job's lease
// runs for, what a holder means in a log line, and — below, in `attempt` — the
// hold to the end of the interval that makes this once-per-interval rather than
// merely not-at-once.

const jobLeaseKey = (name: JobName) => leaseKey('sched', name)

/** Who is holding a lease we failed to take — for the log line only. Returns
 *  'this instance' when the holder is us (i.e. we already ran this interval and
 *  the key is cooling down), 'another instance' when it is not, and null when
 *  the answer could not be read.
 *
 *  The 'self' case reads as "already ran it this interval" ONLY because of this
 *  file's demote-on-completion policy — under the runs policy the same fact
 *  would mean something else entirely — which is why the sentences are written
 *  here and the primitive answers with a bare 'self' / 'other'. */
async function jobLeaseHolder(name: JobName): Promise<string | null> {
  const who = await leaseHolder(jobLeaseKey(name))
  if (who === null) return null
  return who === 'self' ? 'this instance already ran it this interval' : 'another instance holds it'
}

// ── One attempt ──────────────────────────────────────────────────────────────

/** Run one tick of a job. TOTAL: it resolves whatever happens, so a timer can
 *  never leave an unhandled rejection behind, and a failing job never stops its
 *  own schedule. Nothing is swallowed — every branch that gives up says why. */
async function attempt(state: JobState): Promise<void> {
  const { spec } = state
  const { name } = spec

  if (stopping) return

  // Overlap guard, in-process. A job slower than its own interval must not
  // stack up: report the skip (with how long the current run has been going,
  // which is the number you need to decide whether the interval is wrong).
  if (state.running) {
    state.selfOverlaps++
    const forMs = state.startedAt ? Date.now() - state.startedAt : 0
    console.warn(`${LOG} ${name} skipped: previous run still going after ${forMs}ms (skips=${state.selfOverlaps})`)
    return
  }

  const ttlMs = Math.max(5_000, spec.maxRunMs ?? DEFAULT_MAX_RUN_MS)

  // A perInstance job's input is in THIS process's memory (see JobSpec), so
  // there is nothing for another instance to duplicate and nothing for a lease
  // to protect — and taking one would stop every other instance draining its
  // own queue. It also means such a job keeps working when Redis is down,
  // which is right: it is not touching anything Redis is guarding.
  //
  // The token is minted inside `acquireLease` and handed back with the claim: it
  // is unique per ATTEMPT, not per successful run, because it is what the
  // compare-and-set matches on — a token reused across attempts would let a
  // stale one renew a lease that had already expired and been taken by someone
  // else.
  let lease: LeaseToken | null = null
  if (!spec.perInstance) {
    const claim = await acquireLease(jobLeaseKey(name), ttlMs)
    if (claim.kind === 'unavailable') {
      // Fail CLOSED. Running unguarded is how the same chat gets archived twice
      // and the same person gets DMed twice.
      state.leaseSkips++
      console.error(`${LOG} ${name} skipped: Redis lease unavailable (skips=${state.leaseSkips}):`, errText(claim.error))
      return
    }
    if (claim.kind === 'held') {
      // Either someone is running it, or someone already ran it this period.
      state.leaseSkips++
      console.log(`${LOG} ${name} skipped: the lease for this interval is taken (${(await jobLeaseHolder(name)) ?? 'holder unknown'})`)
      return
    }
    lease = claim.token
  }

  state.running = true
  state.startedAt = Date.now()
  state.error = null

  // Keep the lease alive while the job runs, so a job that legitimately takes
  // longer than one TTL is not stolen mid-flight. Two outcomes, two sentences,
  // deliberately: losing the lease means another instance may already be running
  // this job, while a renewal that could not reach Redis usually means the lease
  // is still ours and we simply could not say so.
  const renew: LeaseHeartbeat | null = lease
    ? keepLeaseAlive(lease, ttlMs, {
        onLost: () => console.warn(`${LOG} ${name} lost its lease while running — another instance may have started it`),
        onError: (e) => console.error(`${LOG} ${name} lease renewal failed:`, errText(e)),
      })
    : null

  const done = (async () => {
    try {
      const result = await spec.run()
      state.runs++
      state.result = result
      state.durationMs = Date.now() - (state.startedAt ?? Date.now())
      if (result) console.log(`${LOG} ${name} ok in ${state.durationMs}ms — ${result}`)
      else console.log(`${LOG} ${name} ok in ${state.durationMs}ms — nothing to do`)
    } catch (e) {
      // The whole point of the file: a background failure that nobody ever
      // sees is the same as the work never happening. Name the job, the
      // elapsed time and the error, and keep it in status().
      state.failures++
      state.error = errLine(e)
      state.durationMs = Date.now() - (state.startedAt ?? Date.now())
      console.error(`${LOG} ${name} FAILED after ${state.durationMs}ms (failures=${state.failures}):`, errText(e))
    } finally {
      renew?.stop()
      state.finishedAt = Date.now()
      // Hold the key for the rest of the interval rather than deleting it:
      // that is what makes this "once per interval, fleet-wide" instead of
      // merely "not at the same moment". Anchored to when the run STARTED, not
      // when it finished — a hold measured from the end pushes every period out
      // by the run's own duration, and the cadence drifts a little further
      // behind on every pass. Slightly short of a full interval (0.9) so a tick
      // that arrives a few ms early is not deferred a whole period.
      //
      // A `lost` outcome here is deliberately silent: it means the run outlived
      // its own TTL and the lease has already gone, which the renewal loop
      // above has said out loud once already — and the next tick will simply
      // find the key free and run, which is the correct recovery.
      const startedAt = state.startedAt ?? Date.now()
      const holdMs = Math.max(1, startedAt + Math.floor(spec.everyMs * 0.9) - Date.now())
      if (lease) {
        const held = await demoteLease(lease, holdMs)
        if (held.kind === 'unavailable') {
          console.error(`${LOG} ${name} could not hold its lease for the rest of the interval:`, errText(held.error))
        }
      }
      // Cleared LAST, so a tick that arrives between the run ending and the
      // lease being demoted is turned away by the cheap in-process guard rather
      // than by a Redis round trip.
      state.running = false
    }
  })()

  state.inFlight = done
  try {
    await done
  } finally {
    state.inFlight = null
  }
}

/** The timer callback. Synchronous, so the timer itself can never produce a
 *  rejection; `attempt` is total but the extra catch is deliberate — this is
 *  the boundary where an unhandled rejection would otherwise escape. */
function tick(state: JobState): void {
  void attempt(state).catch((e) => console.error(`${LOG} ${state.spec.name} scheduler tick crashed:`, errText(e)))
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

/** Start every registered job. Idempotent. Returns the names actually armed. */
export function startScheduler(): JobName[] {
  // BEFORE the kill switch, deliberately. A required job that never registered
  // means its module was not in the runtime graph — a fact about this BUILD,
  // not about whether this instance is allowed to run anything. Checking it
  // after the `off` branch meant the one check that catches "a job fell out of
  // the route graph" went silent precisely when an operator is running with the
  // switch thrown to work out what this deployment is doing. Loudly, because
  // the symptom otherwise is "the digest just never arrives" months later.
  const missing = REQUIRED_JOBS.filter((n) => !jobs.has(n))
  if (missing.length) {
    console.error(
      `${LOG} MISSING JOBS: ${missing.join(', ')} did not register. Their module was never imported, so that work will NOT run. This is a bug.`,
    )
  }

  if (process.env.TALARIA_SCHEDULER === 'off') {
    console.warn(`${LOG} disabled by TALARIA_SCHEDULER=off — no background jobs will run on this instance`)
    return []
  }
  if (started) {
    console.warn(`${LOG} startScheduler() called twice — ignoring the second call`)
    return [...jobs.keys()]
  }
  started = true
  stopping = false

  for (const state of jobs.values()) {
    const { spec } = state
    const delay = Math.max(0, spec.firstRunDelayMs ?? 0)
    state.armedAt = Date.now()
    state.firstRunDueAt = state.armedAt + delay
    state.timer = setTimeout(() => {
      state.timer = null
      tick(state)
      state.interval = setInterval(() => tick(state), spec.everyMs)
    }, delay)
  }

  const armed = [...jobs.keys()]
  console.log(
    `${LOG} started on instance ${instanceId} — ${armed.length} job(s): ` +
      armed.map((n) => `${n} every ${Math.round(jobs.get(n)!.spec.everyMs / 1000)}s`).join(', '),
  )
  return armed
}

/** Clear every timer and wait (briefly) for anything in flight. Called on
 *  SIGTERM so a redeploy stops arming new runs the moment it is told to go,
 *  instead of being killed mid-archive. */
export async function stopScheduler(graceMs = 10_000): Promise<void> {
  if (!started) return
  stopping = true
  started = false
  for (const state of jobs.values()) {
    if (state.timer) clearTimeout(state.timer)
    if (state.interval) clearInterval(state.interval)
    state.timer = null
    state.interval = null
  }
  const inFlight = [...jobs.values()].map((s) => s.inFlight).filter((p): p is Promise<void> => !!p)
  if (!inFlight.length) {
    console.log(`${LOG} stopped — no job was in flight`)
    return
  }
  console.log(`${LOG} stopping — waiting up to ${graceMs}ms for ${inFlight.length} job(s) in flight`)
  let bell: ReturnType<typeof setTimeout> | undefined
  const timeout = new Promise<'timeout'>((resolve) => {
    bell = setTimeout(() => resolve('timeout'), graceMs)
  })
  const outcome = await Promise.race([Promise.allSettled(inFlight).then(() => 'done' as const), timeout])
  clearTimeout(bell)
  if (outcome === 'timeout') {
    console.warn(`${LOG} stopped with job(s) still in flight after ${graceMs}ms — their Redis lease will expire on its own`)
  } else {
    console.log(`${LOG} stopped cleanly`)
  }
}

export function schedulerStatus(now = Date.now()): JobStatus[] {
  return [...jobs.values()].map((s) => ({
    name: s.spec.name,
    everyMs: s.spec.everyMs,
    maxRunMs: Math.max(5_000, s.spec.maxRunMs ?? DEFAULT_MAX_RUN_MS),
    running: s.running,
    runningForMs: s.running && s.startedAt ? Math.max(0, now - s.startedAt) : null,
    firstRunDueAt: s.firstRunDueAt ? new Date(s.firstRunDueAt).toISOString() : null,
    runs: s.runs,
    failures: s.failures,
    selfOverlaps: s.selfOverlaps,
    leaseSkips: s.leaseSkips,
    lastStartedAt: s.startedAt ? new Date(s.startedAt).toISOString() : null,
    lastFinishedAt: s.finishedAt ? new Date(s.finishedAt).toISOString() : null,
    lastDurationMs: s.durationMs,
    lastResult: s.result,
    lastError: s.error,
  }))
}

// ── Health, in sentences ─────────────────────────────────────────────────────

export interface JobHealth {
  name: JobName
  severity: 'critical' | 'warning'
  /** One line an operator can act on. */
  detail: string
}

/** Every registered job that is not doing its job, and why.
 *
 *  PROCESS-LOCAL, and that is not a bug to fix here: the counters live in this
 *  process's memory, so on a multi-instance deployment this describes whichever
 *  instance answered the request. That still surfaces a job failing everywhere
 *  (every instance reports it) and a job failing on one box (it appears
 *  intermittently), which is the difference an operator actually needs. A
 *  fleet-wide view needs the counters in Redis, and that is a bigger change
 *  than "make the failure visible at all".
 *
 *  Five things count as unhealthy, in the order they matter:
 *    · the last run threw — the work did not happen;
 *    · the current run has outlived `maxRunMs` — it is HUNG, not slow;
 *    · the job was armed and its first run never landed at all;
 *    · the job is armed but has not completed a run within two intervals of
 *      when it should have;
 *    · the job keeps skipping itself because a run outlives its own interval,
 *      which is the schedule being wrong rather than the code being broken.
 *
 *  The middle two are the M1 cold-boot lesson written down. That wedge was a
 *  job whose FIRST run never returned, and the old checks could not see it from
 *  either side: `failures` was 0 because nothing threw, the lateness check
 *  needed `!running` and `running` was stuck true forever, and the overlap
 *  check needed `runs > 0` when `runs` was 0. So the process was dead, the
 *  scheduler knew the exact reason, and `unhealthyJobs()` reported nothing.
 *  A hang is the failure mode a scheduler has to name out loud, because it is
 *  the only one that produces no error to log. */
export function unhealthyJobs(now = Date.now()): JobHealth[] {
  const out: JobHealth[] = []
  // Operator-facing, so say seconds when it is seconds: "past the 1-minute
  // bound" for a 45s bound is the kind of rounding that makes someone stop
  // trusting the sentence.
  const dur = (ms: number) => (ms < 90_000 ? `${Math.max(1, Math.round(ms / 1_000))}s` : `${Math.round(ms / 60_000)} minutes`)
  // Read through status(), not the private state: this is the consumer that
  // makes JobStatus a real interface rather than a shape nothing has ever
  // needed, and a field that stops being reported here stops being reported at
  // all — which is a thing a reviewer can see.
  for (const s of schedulerStatus(now)) {
    if (s.lastError) {
      out.push({
        severity: 'critical',
        name: s.name,
        detail:
          `the last run failed after ${s.lastDurationMs ?? 0}ms: ${s.lastError}` +
          ` (${s.failures} failure${s.failures === 1 ? '' : 's'} since boot). This work is not happening.`,
      })
      continue
    }
    // HUNG. `maxRunMs` is the job's own declared outside bound for one run, so
    // past it the run is not slow — it is never coming back, and because the
    // overlap guard is doing its job the schedule behind it is stopped dead.
    if (s.running && s.runningForMs !== null && s.runningForMs > s.maxRunMs) {
      out.push({
        severity: 'critical',
        name: s.name,
        detail:
          `has been running for ${dur(s.runningForMs)} — past the ${dur(s.maxRunMs)} bound it declares for one run.` +
          ` It is not coming back on its own, and the schedule behind it is stopped (${s.selfOverlaps} tick(s) skipped since).` +
          ' This work is not happening.',
      })
      continue
    }
    // Only meaningful once the scheduler is actually armed — an un-started
    // registry (vite dev, a test that imported the module) has never been asked
    // to run anything and must not be reported as late.
    const last = s.lastFinishedAt ?? s.lastStartedAt
    // Armed, due, and NOTHING — not even a start. The timer never fired, or it
    // fired into something that never reached `attempt`. There is no `last` to
    // be late against, which is exactly why this needs its own case.
    if (started && !last && s.firstRunDueAt) {
      const overdueMs = now - new Date(s.firstRunDueAt).getTime()
      if (overdueMs > s.everyMs * 2) {
        out.push({
          severity: 'critical',
          name: s.name,
          detail: `was armed at boot and has never started a run — its first was due ${dur(overdueMs)} ago, on a ${dur(s.everyMs)} schedule.`,
        })
        continue
      }
    }
    if (started && !s.running && last) {
      const sinceMs = now - new Date(last).getTime()
      if (sinceMs > s.everyMs * 2) {
        out.push({
          severity: 'warning',
          name: s.name,
          detail: `has not completed a run for ${dur(sinceMs)}, on a ${dur(s.everyMs)} schedule.`,
        })
        continue
      }
    }
    if (s.runs > 0 && s.selfOverlaps > s.runs) {
      out.push({
        severity: 'warning',
        name: s.name,
        detail: `skipped ${s.selfOverlaps} ticks because the previous run was still going (${s.runs} completed). The interval is shorter than the job takes.`,
      })
    }
  }
  return out.sort((a, b) => (a.severity === b.severity ? 0 : a.severity === 'critical' ? -1 : 1))
}

// ── The handle server-entry.js starts us through ─────────────────────────────
//
// server-entry.js is plain JavaScript running the BUILT bundle
// (dist/server/server.js); it cannot import this TypeScript module, and the
// bundle's chunk names are hashed. So this module publishes itself on a
// well-known global symbol as soon as it loads, and the entry — after warming
// the app's server graph — starts it through that handle. If the handle is
// missing, server-entry says so instead of running silently without jobs.
export interface SchedulerHandle {
  start: () => JobName[]
  stop: (graceMs?: number) => Promise<void>
  status: () => JobStatus[]
  health: (now?: number) => JobHealth[]
}

export const SCHEDULER_HANDLE = Symbol.for('talaria.scheduler')
;(globalThis as unknown as Record<symbol, SchedulerHandle>)[SCHEDULER_HANDLE] = {
  start: startScheduler,
  stop: stopScheduler,
  status: schedulerStatus,
  health: unhealthyJobs,
}
