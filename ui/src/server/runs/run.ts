// THE driver. One chokepoint that turns a `RunDefinition` into a durable run:
// it takes the lease, loops `step()`, persists every checkpoint, parks on a
// human decision, and gives the lease back — and it is the only code in the
// runs system that writes a state transition.
//
// WHAT IT GUARANTEES, and what it deliberately does not:
//
//   IT SURVIVES THE PROCESS. Nothing about a run lives in this process except
//   the lease token and the current step's promise. A restart, a deploy, a
//   container paused past its lease: the row is still there, still `running`,
//   with its last checkpoint, and the next sweep re-enters `step()` with it.
//   Compare server/research.ts:352, which answers the same event by marking the
//   user's run FAILED with 'run went stale (app restarted mid-research?)'.
//
//   IT IS AT-LEAST-ONCE, and this is the risk every caller must hold. A
//   reclaimed run RE-ENTERS `step()` with the last PERSISTED checkpoint. A step
//   that ran and did not persist runs AGAIN — if it archived a chat, sent a DM,
//   opened a PR or billed a model call, that happens twice. The driver's own
//   ordering rule (below) makes the window as small as a database write, and no
//   smaller. It cannot be closed from here: only the step knows whether its
//   side effect is repeatable, and only the step can guard it.
//
//   A LOST LEASE IS A CLEAN STOP. Not an error, not a failure, not a
//   notification: another instance owns this run now, and the correct behavior
//   is to stop touching it and say so at log level. Every write in store.ts is
//   a compare-and-set on the lease token, so a driver that missed the news
//   cannot write anyway.
//
//   CANCELLATION IS HONORED BY ANY INSTANCE. The driver re-reads the row at
//   every step boundary and every write requires `state = 'running'`, so a
//   cancel issued on a different instance stops this one at the next boundary.
//   That is the whole difference from server/fitness/surface.ts, whose stop is a
//   module-level boolean that only works on the process that happens to be
//   running the sweep.
//
//   A THROWN STEP IS AN ERROR ROW WITH THE MESSAGE ON IT. Never swallowed,
//   never a silent `return null`, never a state left at `running` for a sweep
//   to reinterpret later. "A return null nobody hears about" is the exact
//   disease this project has spent the audit eliminating.
//
// TESTABILITY IS A DESIGN CONSTRAINT. Every edge to the outside world — the
// store, the lease, the publish, the park, the clock, the id generator, the
// definition registry — is a field on `RunDeps`, defaulted to the real thing
// and overridable per call. run.test.ts drives the whole runner with no
// database, no Redis and no clock. Same pattern, same reason, as
// server/harness/run.ts.
import { randomUUID } from 'node:crypto'
import { audienceFor, type Authority, type Disclosure } from '../approvals'
import { publishRun as publishRunTopic, publishUser } from '../realtime'
import { errLine, errText } from '../errors'
import type { PauseResult } from './decide'
import { acquireRunLease, releaseRunLease, renewLease, runLeaseKey, type LeaseToken } from './lease'
import {
  DEFAULT_MAX_ATTEMPTS,
  isDrivable,
  runDefinition,
  type AnyRunDefinition,
  type DecisionAnswer,
  type DecisionRequest,
  type RunDefinition,
  type RunRow,
  type RunState,
  type RunStepContext,
  type StepResult,
} from './define'
import { pgRunStore, type RunStore } from './store'

const LOG = '[runs]'

// ── The lease, as the driver needs it ────────────────────────────────────────
//
// The mutual exclusion itself lives in runs/lease.ts — one primitive, shared
// with the scheduler, with the compare-and-set scripts and the token format in
// one place. This interface is the narrow seam the DRIVER holds it through:
// three verbs, keyed by run id, with the token as a plain string because that
// same string is also the row's `lease_owner`. One identity, in Redis and in
// Postgres, is what makes "is this run still mine" answerable from either side.

export type LeaseClaim =
  | { ok: true; token: string }
  /** Another instance is stepping this run right now. Not an error. */
  | { ok: false; reason: 'busy' }
  /** Redis could not be asked, so this process cannot know whether it is alone.
   *  LEAVE THE ROW ALONE: do not step it, and above all do not mark it failed.
   *  The checkpoint is durable and a later sweep will claim it. */
  | { ok: false; reason: 'blocked'; error: unknown }

/** `lost` and `unavailable` are kept apart on purpose: one means another
 *  instance owns the run now, the other means we could not say. Both stop this
 *  driver; they are different sentences in the log, and collapsing them is how
 *  a Redis blip gets read as a handover for the rest of the incident. */
export type LeaseRenewal = 'ok' | 'lost' | 'unavailable'

export interface RunLease {
  /** Claim the run for ONE STEP's worth of time. Never throws. */
  acquire(runId: string, stepMs: number): Promise<LeaseClaim>
  renew(runId: string, token: string, stepMs: number): Promise<LeaseRenewal>
  /** Compare-and-delete. A lease released by anyone but its owner would be a
   *  lease that a slow driver deletes out from under its successor. */
  release(runId: string, token: string): Promise<void>
}

/** Reconstitute the lease module's token from the string the driver carries. It
 *  is a value, not a handle — the whole point of the format is that any process
 *  holding the string can prove ownership by compare-and-set. */
const tokenFor = (runId: string, value: string): LeaseToken => ({ key: runLeaseKey(runId), value })

export const runLease: RunLease = {
  async acquire(runId, stepMs) {
    const claim = await acquireRunLease(runId, stepMs)
    if (claim.kind === 'claimed') return { ok: true, token: claim.lease.value }
    if (claim.kind === 'busy') return { ok: false, reason: 'busy' }
    return { ok: false, reason: 'blocked', error: claim.error }
  },
  async renew(runId, token, stepMs) {
    const res = await renewLease(tokenFor(runId, token), stepMs)
    return res.kind === 'ok' ? 'ok' : res.kind === 'lost' ? 'lost' : 'unavailable'
  },
  async release(runId, token) {
    // Failure here is survivable and must not mask the run's own outcome: the
    // key carries a TTL, so the worst case is that the next driver waits it out.
    const res = await releaseRunLease(tokenFor(runId, token))
    if (res.kind === 'unavailable') console.error(`${LOG} could not release the lease for ${runId}:`, errText(res.error))
  },
}

// ── What a device sees ───────────────────────────────────────────────────────

/** The event published on every persisted transition. Small on purpose: it
 *  carries enough to render a list row and to decide whether to refetch, and
 *  nothing that could disagree with the table. A payload that duplicated the
 *  checkpoint is a payload that will be stale on arrival. */
export interface RunEvent {
  type: 'run'
  runId: string
  kind: string
  state: RunState
  phase: string
  /** Present only on the transition into `awaiting`, so a device can raise the
   *  question without a round trip. */
  question?: DecisionRequest
  error?: string
}

/** THE fan-out, and the only one. Both topics belong to server/realtime.ts,
 *  which owns every pub/sub topic in the product and the SSE streams that read
 *  them: `run:<id>` is "how is THIS run doing" (a detail view, a progress
 *  strip) and `user:<id>` is "what do I have in flight" — the per-person
 *  firehose a SECOND DEVICE attaches to, which must update for a run the person
 *  is not looking at, because the transition that matters most (`awaiting`) is
 *  exactly the one that happens while their attention is elsewhere.
 *
 *  THIS FILE PUBLISHED ITS OWN PAIR OF TOPICS FOR ONE ROUND OF THIS PROJECT and
 *  it is worth naming the bug that cost, because it is the kind nobody finds by
 *  reading either file alone: the driver published the per-user event on
 *  `runs:user:<id>` while /api/me/events subscribed to `user:<id>`. Two agents,
 *  two spellings, one dead stream — a second device that attaches, connects
 *  successfully, and simply never hears anything. One publisher is the fix.
 *
 *  THE QUESTION DOES NOT GO ON THE WIRE. `RunEvent` above carries it inside
 *  this process so the park path can hand it straight to `pause`, but who may
 *  READ a decision's text is the definition's `audience` (an approvals
 *  Authority) while who may WATCH a run is its subject's read ACL — different
 *  sets, overlapping in the common case and not guaranteed to. The fields are
 *  named one at a time on the way out so `question` cannot ride along by
 *  accident; a device that sees `awaiting` re-fetches through the route that
 *  resolves the audience properly. */
export function publishRunEvent(event: RunEvent, ownerUserId: string | null): void {
  publishRunTopic(event.runId, {
    type: 'run',
    runId: event.runId,
    kind: event.kind,
    state: event.state,
    phase: event.phase,
    ...(event.error === undefined ? {} : { error: event.error }),
  })
  if (ownerUserId) publishUser(ownerUserId, { type: 'run', runId: event.runId, state: event.state })
}

// ── Deps ─────────────────────────────────────────────────────────────────────

export interface RunDeps {
  store: RunStore
  lease: RunLease
  publish: (event: RunEvent, ownerUserId: string | null) => void
  /** THE `running → awaiting` transition — park the run on the question, file
   *  it as an approval, tell whoever the definition's `audience` names.
   *
   *  THE DRIVER DOES NOT WRITE IT ITSELF, and that is a correction rather than
   *  a preference. This file and server/runs/decide.ts each grew their own copy
   *  of the same four steps (derive the approval key, `store.park`, publish,
   *  announce) and the copies had already begun to differ — a second spelling
   *  of the key would have announced one pause twice, and only one of them
   *  dropped the question before the wire. The park lives in decide.ts because
   *  that is where the other half lives: `pause` and `decide` are the two ends
   *  of one transition, and rules about who may be told what a question says
   *  belong next to the rules about who may answer it.
   *
   *  Imported at CALL TIME, not at module load: decide.ts imports this module
   *  for `drive` and the shared types, so a static import back is a cycle. Same
   *  reason (and the same shape) as the deferred imports in server/realtime.ts. */
  pause: (
    args: { runId: string; token: string; question: DecisionRequest; phase?: string },
    deps: Partial<RunDeps> & { announce?: (approvalKey: string) => Promise<number> },
  ) => Promise<PauseResult>
  /** Resolve an authority to who may be told, and how much. Read by `pause`
   *  through this same bag, which is why it stays on the driver's deps: one
   *  object describes one world, and a test that faked the store here and hit
   *  the real resolver over there would be testing neither. */
  audienceFor: (authority: Authority) => Promise<Disclosure>
  definitionFor: (kind: string) => AnyRunDefinition | null
  now: () => number
  newId: () => string
}

const REAL_DEPS: RunDeps = {
  store: pgRunStore,
  lease: runLease,
  publish: publishRunEvent,
  pause: async (args, deps) => (await import('./decide')).pause(args, deps),
  audienceFor,
  definitionFor: runDefinition,
  now: () => Date.now(),
  newId: () => randomUUID(),
}

const withDeps = (deps: Partial<RunDeps>): RunDeps => ({ ...REAL_DEPS, ...deps })

// ── Enqueue ──────────────────────────────────────────────────────────────────

export interface EnqueueOptions {
  /** Whose run it is. Null for org-wide work with nobody behind it. */
  ownerUserId?: string | null
  /** What it is about — 'task' / 'channel' / 'conversation' / 'research'. */
  subjectType?: string | null
  subjectId?: string | null
  /** The first line a waiting human reads, before the first `ctx.log`. */
  phase?: string
  /** Supply the id, for a caller that must reference the run in the same
   *  transaction it creates it from. */
  id?: string
  /** Begin driving immediately, detached from this request. Default true.
   *
   *  A NICETY, NOT THE GUARANTEE. The detached drive is what makes a run start
   *  in the same second the button was pressed; the RECLAIM SWEEP is what makes
   *  it finish. If this process dies between the insert and the first
   *  checkpoint, the row is `queued` with no lease and the sweep takes it — no
   *  part of the durability story depends on this promise being awaited, which
   *  is exactly what routes/api/chat.ts's detached persist gets right and
   *  everything else that copied the idea got wrong. */
  start?: boolean
}

/** Write the row, publish it, return. Never waits for the work.
 *
 *  Returns the row rather than an id because the caller almost always wants to
 *  render it immediately, and a caller that has to re-read what it just wrote
 *  is a caller that will render a state the database has not got round to. */
export async function enqueue<I, C>(
  def: RunDefinition<I, C>,
  input: I,
  opts: EnqueueOptions = {},
  deps: Partial<RunDeps> = {},
): Promise<RunRow> {
  const d = withDeps(deps)
  // A run whose kind nothing has registered can be started by THIS process and
  // then never resumed by any other — the sweep would find the row and have no
  // code to advance it. That is a wiring bug at module load, so say it at
  // enqueue time when the stack still names the caller.
  if (!d.definitionFor(def.kind))
    console.error(
      `${LOG} enqueuing kind "${def.kind}", which is not registered — it cannot be reclaimed after a restart. ` +
        `Call registerRun(def) at module load.`,
    )

  const run = await d.store.insert({
    id: opts.id ?? d.newId(),
    kind: def.kind,
    ownerUserId: opts.ownerUserId ?? null,
    subjectType: opts.subjectType ?? null,
    subjectId: opts.subjectId ?? null,
    input,
    phase: opts.phase ?? 'queued',
  })

  // THE ORDERING RULE, first instance: the row exists before anybody is told it
  // does. A publish that outran the write is how a second device renders a run
  // the database does not have — it refetches, gets a 404, and shows an error
  // for a run that is perfectly healthy.
  d.publish({ type: 'run', runId: run.id, kind: run.kind, state: run.state, phase: run.phase }, run.ownerUserId)

  if (opts.start !== false) {
    void drive(run.id, deps).catch((e) => console.error(`${LOG} detached drive of ${run.id} threw:`, errText(e)))
  }
  return run
}

// ── Drive ────────────────────────────────────────────────────────────────────

/** Why this drive stopped. Six of these are perfectly healthy, and the point of
 *  spelling them out is that a caller (and a log line) can tell them apart. */
export type DriveStop =
  | 'done'
  | 'error'
  /** Parked on a human decision. */
  | 'awaiting'
  /** A `retry` result: scheduled, nobody bothered, no attempt consumed. */
  | 'deferred'
  | 'cancelled'
  /** Another instance owns it now. Clean. */
  | 'lease-lost'
  /** Somebody else is driving it, or a deferral has not elapsed. Clean. */
  | 'busy'
  /** Redis could not be asked whether this driver would be alone, so the row
   *  was left ALONE — not driven, and above all not failed. */
  | 'blocked'
  | 'missing'
  /** Nothing in THIS process knows this kind. Not a failure of the run. */
  | 'no-definition'
  /** Reclaimed more times than the definition allows; filed as an error. */
  | 'exhausted'
  /** The row is in a state no driver advances (`awaiting`, or terminal). */
  | 'not-runnable'

export interface DriveResult {
  runId: string
  stop: DriveStop
  /** How many times `step()` was entered on THIS drive. */
  steps: number
  state: RunState | null
  error?: string
  /** For `deferred`: how long until it may be taken again. */
  retryAfterMs?: number
}

/** Thrown into the step race when the deadline passes or the lease is lost.
 *  Not exported: nothing outside this loop should be able to fake one. */
class StepInterrupted extends Error {
  constructor(readonly why: 'deadline' | 'lease-lost') {
    super(why === 'deadline' ? 'step exceeded maxStepMs' : 'lease lost while the step was running')
    this.name = 'StepInterrupted'
  }
}

/** Take the run and advance it until it finishes, pauses, or stops being ours.
 *
 *  Safe to call from anywhere, on any instance, as often as you like: a run
 *  somebody else is driving comes back `busy` after one Redis round trip. */
export async function drive(runId: string, deps: Partial<RunDeps> = {}): Promise<DriveResult> {
  const d = withDeps(deps)

  const existing = await d.store.get(runId)
  if (!existing) return { runId, stop: 'missing', steps: 0, state: null }
  const def = d.definitionFor(existing.kind)
  if (!def) {
    // NOT an error on the row. A kind this instance has never imported — a row
    // from a newer deploy, or a module not in this process's graph — is still
    // perfectly drivable by an instance that has it, and failing the run here
    // would destroy work on the strength of a local import graph.
    console.warn(`${LOG} ${runId}: no definition registered for kind "${existing.kind}" on this instance — leaving it for one that has it`)
    return { runId, stop: 'no-definition', steps: 0, state: existing.state }
  }
  // `isDrivable`, not a state comparison spelled out again here. There is one
  // rule about which states a driver may pick up and `awaiting` is the reason it
  // is worth centralizing: a run parked on a person is healthy, not stuck, and
  // the day somebody adds a seventh state the wrong copy of this predicate is
  // the one that will not be updated. reclaim.ts calls the same function.
  if (!isDrivable(existing.state)) return { runId, stop: 'not-runnable', steps: 0, state: existing.state }

  // The lease TTL is the step budget: a driver that dies mid-step is
  // reclaimable one step-length after it stops renewing, which is the shortest
  // safe answer available without guessing. The floor keeps a definition with a
  // very small `maxStepMs` from making its own lease unrenewable.
  const leaseMs = Math.max(5_000, def.maxStepMs)

  const claimed = await d.lease.acquire(runId, leaseMs)
  if (!claimed.ok) {
    if (claimed.reason === 'busy') return { runId, stop: 'busy', steps: 0, state: existing.state }
    // FAIL CLOSED, exactly as the scheduler does. An unreachable Redis is not
    // permission to run unguarded; two instances advancing one run is how the
    // side effect happens twice. The row is untouched — not failed, not
    // rewritten — so a later sweep takes it from the same checkpoint.
    console.error(`${LOG} ${runId}: lease unavailable, not driving:`, errText(claimed.error))
    return { runId, stop: 'blocked', steps: 0, state: existing.state, error: errLine(claimed.error) }
  }
  const token = claimed.token

  const claim = await d.store.claim({ id: runId, token, leaseMs })
  if (!claim.ok) {
    // The row disagreed with the lease. Give the lease straight back rather
    // than letting it time out, or a run that was merely raced would be
    // unclaimable for a whole TTL.
    await d.lease.release(runId, token)
    if (claim.reason === 'missing') return { runId, stop: 'missing', steps: 0, state: null }
    if (claim.reason === 'taken') return { runId, stop: 'busy', steps: 0, state: claim.state }
    return { runId, stop: 'not-runnable', steps: 0, state: claim.state }
  }

  let row = claim.run
  const maxAttempts = def.maxAttempts ?? DEFAULT_MAX_ATTEMPTS

  // `attempt` counts ENTRIES that followed a crash, so this is "how many
  // drivers has this run killed". A run over the line is filed as an error with
  // the count in the message: it is a bug report, and a bug report that reads
  // 'failed' with no number is one nobody can act on.
  if (row.attempt >= maxAttempts) {
    const message = `run gave up after ${row.attempt} attempt(s): each driver that took it stopped without finishing or checkpointing`
    console.error(`${LOG} ${runId} (${row.kind}): ${message}`)
    const write = await d.store.fail({ id: runId, token, error: message })
    await d.lease.release(runId, token)
    if (write.ok) d.publish({ type: 'run', runId, kind: row.kind, state: 'error', phase: row.phase, error: message }, row.ownerUserId)
    return { runId, stop: 'exhausted', steps: 0, state: 'error', error: message }
  }

  if (claim.reclaimed)
    console.warn(
      `${LOG} ${runId} (${row.kind}) reclaimed at attempt ${row.attempt} of ${maxAttempts}, phase "${row.phase}" — ` +
        `the previous driver stopped without releasing. Its last step re-runs from the persisted checkpoint.`,
    )

  // ── Lease renewal ──────────────────────────────────────────────────────────
  // TWO WRITES PER BEAT, and both matter: Redis is what another instance TESTS
  // before it takes the run, and the row's `lease_expires_at` is what the
  // reclaim query SCANS (Redis has no index over "every run whose lease
  // expired"). That pairing is why the driver runs its own interval instead of
  // `keepRunLeaseAlive` — the lease module's heartbeat pumps Redis, correctly,
  // and knows nothing about the row that has to agree with it.
  //
  // Any beat that does not come back `ok` aborts the step and stops the drive.
  // A renewal that cannot REACH Redis has not necessarily lost the lease, but
  // it can no longer prove it holds one, and continuing to write on a lease we
  // cannot verify is the one thing that turns a blip into two drivers.
  let lostLease = false
  let current: AbortController | null = null
  const renewEvery = Math.max(1_000, Math.floor(leaseMs / 3))
  const renewTimer = setInterval(() => {
    void (async () => {
      const renewed = await d.lease.renew(runId, token, leaseMs)
      const beat = renewed === 'ok' ? await d.store.heartbeat({ id: runId, token, leaseMs }) : ({ ok: false } as const)
      if (renewed === 'ok' && beat.ok) return
      lostLease = true
      if (renewed === 'unavailable') console.error(`${LOG} ${runId}: could not renew the lease (Redis unreachable), stopping — a later sweep resumes it`)
      else console.warn(`${LOG} ${runId}: lost the lease mid-run — another instance owns it now, stopping cleanly`)
      current?.abort(new StepInterrupted('lease-lost'))
    })().catch((e) => {
      // A timer callback that rejects is an unhandled rejection with no stack
      // worth reading, and it must never be the thing that takes the process
      // down. The store's heartbeat is the only await above that can throw.
      lostLease = true
      console.error(`${LOG} ${runId}: heartbeat failed, stopping:`, errText(e))
      current?.abort(new StepInterrupted('lease-lost'))
    })
  }, renewEvery)

  // ── Progress lines ─────────────────────────────────────────────────────────
  // `ctx.log` is fire-and-forget for the step, but it is NOT unordered: each
  // call chains onto the last, persists, and only then publishes, and the loop
  // awaits the chain at every boundary. A phase line that outran its write is a
  // device showing a sentence the database will never confirm.
  let logChain: Promise<void> = Promise.resolve()
  let phase = row.phase
  const log = (next: string): void => {
    const text = next.slice(0, 300)
    phase = text
    logChain = logChain
      .then(async () => {
        const write = await d.store.phase({ id: runId, token, phase: text })
        if (!write.ok) return // the boundary check reports why; a log line does not get to shout about it
        d.publish({ type: 'run', runId, kind: row.kind, state: 'running', phase: text }, row.ownerUserId)
      })
      .catch((e) => console.error(`${LOG} ${runId}: could not record phase "${text}":`, errText(e)))
  }

  let steps = 0
  /** Set when the run is deferred, so the finally block HOLDS the lease for the
   *  wait instead of releasing it — releasing would let another instance take
   *  the run immediately and defeat the whole point of `retry.after`. */
  let deferMs: number | null = null

  try {
    for (;;) {
      // ── The step boundary ────────────────────────────────────────────────
      // Everything that can stop this drive is checked HERE, before any work is
      // done, and the row is re-read from the store rather than carried in
      // memory. That read is worth its round trip twice over: it is the
      // cancellation check (any instance, honored here), and it means the
      // checkpoint handed to the next step is the PERSISTED one — resume is the
      // same code path as ordinary progress, not a second one that could drift.
      await logChain
      if (lostLease) return { runId, stop: 'lease-lost', steps, state: row.state }

      const fresh = await d.store.get(runId)
      if (!fresh) return { runId, stop: 'missing', steps, state: null }
      row = fresh
      if (row.state === 'cancelled') {
        console.log(`${LOG} ${runId} (${row.kind}): cancelled, stopping at step ${steps}`)
        return { runId, stop: 'cancelled', steps, state: row.state }
      }
      if (row.leaseOwner !== token) {
        lostLease = true
        return { runId, stop: 'lease-lost', steps, state: row.state }
      }
      if (row.state !== 'running') return { runId, stop: 'not-runnable', steps, state: row.state }

      // ── One step ─────────────────────────────────────────────────────────
      const ac = new AbortController()
      current = ac
      const answer: DecisionAnswer | null = row.decision?.answer ?? null
      const ctx: RunStepContext<unknown, unknown> = {
        run: row,
        input: row.input,
        checkpoint: row.checkpoint ?? null,
        decision: answer,
        signal: ac.signal,
        log,
        attempt: row.attempt,
      }

      const deadline = setTimeout(() => ac.abort(new StepInterrupted('deadline')), def.maxStepMs)
      let result: StepResult<unknown>
      try {
        steps++
        result = await Promise.race([
          def.step(ctx),
          // The abort is raced rather than merely signalled because a step is
          // allowed to ignore `ctx.signal` — and the driver still has to stop.
          // See the deadline branch below for what that abandonment costs.
          new Promise<never>((_, reject) => {
            if (ac.signal.aborted) {
              reject(ac.signal.reason instanceof StepInterrupted ? ac.signal.reason : new StepInterrupted('deadline'))
              return
            }
            ac.signal.addEventListener(
              'abort',
              () => reject(ac.signal.reason instanceof StepInterrupted ? ac.signal.reason : new StepInterrupted('deadline')),
              { once: true },
            )
          }),
        ])
      } catch (e) {
        clearTimeout(deadline)
        current = null
        if (e instanceof StepInterrupted && e.why === 'lease-lost') return { runId, stop: 'lease-lost', steps, state: row.state }
        // A step that blew its own declared budget. It is filed as an error and
        // NOT retried, and the reason is that the step is very probably STILL
        // RUNNING: nothing here can stop a promise that ignores its signal, so
        // re-entering the run would put two copies of the same step in flight
        // inside one process — the doubled side effect this system exists to
        // prevent, caused by the machinery meant to prevent it. `maxStepMs` is
        // the definition's OWN statement of how long a unit of progress takes;
        // blowing it is a bug, and a visible error row is how a bug gets fixed.
        const message =
          e instanceof StepInterrupted
            ? `step exceeded maxStepMs (${def.maxStepMs}ms) at phase "${phase}". The step may still be running; it will not be retried.`
            : errLine(e)
        if (!(e instanceof StepInterrupted)) console.error(`${LOG} ${runId} (${row.kind}) step threw at phase "${phase}":`, errText(e))
        else console.error(`${LOG} ${runId} (${row.kind}): ${message}`)
        await logChain
        const write = await d.store.fail({ id: runId, token, error: message })
        if (!write.ok) return stopFromWrite(runId, steps, write.reason, write.reason === 'cancelled' ? 'cancelled' : row.state)
        d.publish({ type: 'run', runId, kind: row.kind, state: 'error', phase, error: message }, row.ownerUserId)
        return { runId, stop: 'error', steps, state: 'error', error: message }
      }
      clearTimeout(deadline)
      current = null
      await logChain

      // ── Apply the result ─────────────────────────────────────────────────
      //
      // THE ORDERING RULE, everywhere below: PERSIST, THEN PUBLISH. A publish
      // that outruns its write is how a second device renders state the
      // database does not have — the user's other tab shows "done", refetches,
      // and gets a run that is still running. And where the ordering allows it,
      // persist BEFORE the side effect: the checkpoint write below happens
      // before the next step (which is where the side effects live), so a crash
      // costs at most the one step that had not checkpointed yet.
      if (result.kind === 'next') {
        if (result.phase) phase = result.phase.slice(0, 300)
        const write = await d.store.checkpoint({
          id: runId,
          token,
          checkpoint: result.checkpoint,
          phase,
          // Clear the answer the step just consumed, in the SAME write that
          // records the progress it produced. Two writes would leave a window
          // where a reclaim hands the next step an answer to a question that
          // has already been acted on, and it would act on it again.
          clearDecision: answer !== null,
        })
        if (!write.ok) return stopFromWrite(runId, steps, write.reason, write.reason === 'cancelled' ? 'cancelled' : row.state)
        d.publish({ type: 'run', runId, kind: row.kind, state: 'running', phase }, row.ownerUserId)
        continue
      }

      if (result.kind === 'done') {
        const write = await d.store.complete({ id: runId, token, result: result.result ?? null })
        if (!write.ok) return stopFromWrite(runId, steps, write.reason, write.reason === 'cancelled' ? 'cancelled' : row.state)
        d.publish({ type: 'run', runId, kind: row.kind, state: 'done', phase }, row.ownerUserId)
        return { runId, stop: 'done', steps, state: 'done' }
      }

      if (result.kind === 'decide') {
        // ONE PARK, and it is not here. `pause` derives the approval key, does
        // the compare-and-set park, publishes and announces — see the note on
        // `RunDeps.pause`. It cannot fail the run: every refusal it returns is
        // a normal outcome (somebody cancelled it, somebody else took it, the
        // row went away) and maps onto the same stops as any other refused
        // write, while a delivery that did not happen leaves the row `awaiting`
        // with an UNMARKED key for the approvals sweep to find.
        const parked = await d.pause({ runId, token, question: result.question, phase }, deps)
        if (!parked.ok) return stopFromWrite(runId, steps, parked.reason, parked.reason === 'cancelled' ? 'cancelled' : row.state)
        return { runId, stop: 'awaiting', steps, state: 'awaiting' }
      }

      // `retry`: a SOFT pause. No notification, no attempt consumed, state back
      // to `queued` — the wait is expressed as a lease that has not expired
      // yet, which is the same thing every other "not yet" in this file means.
      const after = Math.max(0, Math.floor(result.after))
      const until = d.now() + after
      const write = await d.store.defer({ id: runId, token, until, reason: result.reason.slice(0, 300) })
      if (!write.ok) return stopFromWrite(runId, steps, write.reason, write.reason === 'cancelled' ? 'cancelled' : row.state)
      deferMs = after
      d.publish({ type: 'run', runId, kind: row.kind, state: 'queued', phase: result.reason.slice(0, 300) }, row.ownerUserId)
      return { runId, stop: 'deferred', steps, state: 'queued', retryAfterMs: after }
    }
  } finally {
    clearInterval(renewTimer)
    if (deferMs !== null) {
      // HOLD the lease for the wait. Releasing it would let the next sweep take
      // the run immediately, turning "come back in thirty seconds" into a hot
      // loop against whatever asked for the wait in the first place.
      const held = await d.lease.renew(runId, token, Math.max(1, deferMs))
      if (held !== 'ok') console.warn(`${LOG} ${runId}: could not hold the deferral in Redis (${held}) — the row's own expiry still gates it`)
    } else {
      // Give it back rather than letting it time out: a run this driver
      // finished, parked or handed over should be takeable now, not one TTL
      // from now. `release` is compare-and-delete, so a lease that already
      // moved on is left alone.
      await d.lease.release(runId, token)
      // And drop the row's lease stamp, but ONLY if it is still ours and still
      // running — every terminal write above already cleared it, so this is the
      // clean-stop path (cancelled, lease-lost, a row that moved under us).
      await d.store.release({ id: runId, token }).catch((e) => console.error(`${LOG} ${runId}: could not clear the row lease:`, errText(e)))
    }
  }
}

/** Map a refused compare-and-set onto a stop. Every branch is a NORMAL outcome
 *  — somebody cancelled it, somebody else took it, the row went away — and none
 *  of them is an error the run should be marked with. */
function stopFromWrite(runId: string, steps: number, reason: 'lease-lost' | 'cancelled' | 'missing' | 'state', state: RunState): DriveResult {
  if (reason === 'cancelled') {
    console.log(`${LOG} ${runId}: cancelled while a step was running — the step's result is discarded`)
    return { runId, stop: 'cancelled', steps, state: 'cancelled' }
  }
  if (reason === 'missing') return { runId, stop: 'missing', steps, state: null }
  if (reason === 'lease-lost') {
    console.log(`${LOG} ${runId}: another instance owns this run now, stopping cleanly`)
    return { runId, stop: 'lease-lost', steps, state }
  }
  return { runId, stop: 'not-runnable', steps, state }
}

// ── The sweep, and the answer, both deliberately absent ──────────────────────
//
// THE RECLAIM SWEEP IS server/runs/reclaim.ts. This file grew a second, thinner
// one (`reclaimDueRuns`) before that module landed, and two sweepers over one
// table is not a redundancy — it is two different answers to "may this row be
// woken", which is the one question in this system that must have exactly one.
// The thin one had no `awaiting` guard of its own, no bound worth the name, and
// no report; it survived only because the store's WHERE clause happened to
// agree with it. reclaim.ts is the registered job, it re-states the `awaiting`
// guard in code rather than trusting an index definition, and it reports what a
// pass did. Anything that wants runs resumed calls `sweepReclaimableRuns`.
//
// THE ANSWER TO A PARKED QUESTION IS server/runs/decide.ts. This file also grew
// an `answerRun` — `store.answer` plus a publish plus a detached drive, with no
// authority check anywhere in it. That is the ONLY write that takes a row out
// of `awaiting`, and an exported, ungated version of it beside the gated one
// means the gate is a convention: any route could import the wrong name and
// resume a run on behalf of somebody entitled to nothing. The house rule is
// `logTicket` in server/workbench-mcp.ts — a caller must not be able to EXPRESS
// the ungated write — so that function is now module-private inside decide.ts,
// behind `decide()`, which resolves the definition's authority and asks
// `mayDecide` before it writes.

/** Stop a run, from anywhere. The driver that owns it finds out at its next
 *  step boundary — or when its next write is refused, whichever comes first. */
export async function cancelRun(
  args: { runId: string; reason?: string },
  deps: Partial<RunDeps> = {},
): Promise<{ ok: true; state: RunState } | { ok: false; reason: 'missing' | 'terminal'; state?: RunState }> {
  const d = withDeps(deps)
  const res = await d.store.cancel({ id: args.runId, reason: args.reason })
  if (!res.ok) return res
  const run = await d.store.get(args.runId)
  if (run) d.publish({ type: 'run', runId: run.id, kind: run.kind, state: run.state, phase: run.phase }, run.ownerUserId)
  return res
}

/** What this person has in flight, for the strip and the list. */
export async function activeRuns(userId: string, deps: Partial<RunDeps> = {}): Promise<RunRow[]> {
  return withDeps(deps).store.activeFor({ userId })
}

export { pgRunStore, type RunStore } from './store'
