// The lease — the one mechanism in this tree for "exactly one process is doing
// this right now", and the two POLICIES built on it.
//
// WHY THIS FILE EXISTS
//   The scheduler wrote a Redis lease because duplication there is not a wasted
//   CPU cycle, it is user-visible harm: comms decay ARCHIVES people's
//   conversations and the outreach sweep SENDS MESSAGES, so a second instance
//   doing the same pass archives the same chat twice and DMs the same person
//   twice. Everything else that needed the same guarantee reached for a
//   process-local Set instead — `liveSessions` in work-dispatch.ts (with its own
//   TODO(multi-instance) admitting it), `continuing` in chat-persist.ts,
//   `locks` in inbox-focus-conversation.ts, `inFlight` in price-oracle.ts. Every
//   one of those is correct on one instance and wrong the moment there are two,
//   and none of them survives a restart.
//
//   So the lease moves here, whole, and the runs runtime takes the same
//   primitive rather than writing a fifth guard. scheduler.ts keeps its own
//   policy — see below, the policies are genuinely different and forcing one on
//   both would break the scheduler.
//
// THE PRIMITIVE, and why every piece of it is load-bearing
//   · SET NX PX          take the key only if nobody has it, and never hold it
//                        forever. A lease with no TTL is a deadlock waiting for
//                        the one process that can clear it to be the one that
//                        crashed.
//   · a TOKEN            the value written under the key identifies the holder,
//                        and every subsequent operation is a COMPARE-AND-SET
//                        against it. Without that, a process whose lease expired
//                        mid-work would go on renewing (and eventually deleting)
//                        a lease another instance now legitimately holds — which
//                        is worse than never having leased at all, because both
//                        processes believe they are alone.
//   · minted HERE        `acquireLease` returns the token; callers never supply
//                        one. "Only the process that took the lease may renew or
//                        release it" is then a fact about the API rather than a
//                        rule someone has to remember, and a caller cannot pass
//                        a stale token from a previous attempt.
//
// TWO POLICIES OVER ONE PRIMITIVE — the reason this is an extraction and not a
// move. Do not try to unify them; the difference is the whole point.
//
//   THE SCHEDULER LEASES A NAMED JOB FOR A PERIOD. It deliberately holds the key
//   PAST completion (`demoteLease`), because a mutex only stops the two
//   instances running the job at the SAME time, and would still let instance B
//   run comms decay a minute after instance A finished — twice per interval. The
//   key is the period's receipt, not a mutex.
//
//   A RUN LEASES A ROW FOR THE DURATION OF ONE STEP and releases it immediately
//   (`releaseRunLease`), because the next step should be claimable by ANY
//   instance. Holding it past the step would pin a long run to whichever
//   instance happened to start it, and that instance's redeploy would stall the
//   run until the TTL ran out.
//
//   And the expiries mean opposite things. A scheduler lease expiring
//   un-demoted means the run died and the job is due again. A RUN LEASE EXPIRING
//   IS NOT A FAILURE — IT IS A RECLAIM SIGNAL: the row is still there, still
//   holding its last persisted checkpoint, and another instance should pick it
//   up from there. That is why nothing in here marks anything failed.
//
// REDIS UNREACHABLE — decided deliberately for each, because "fail closed"
// spells differently for a job and for a run.
//
//   The scheduler SKIPS the tick. Its work is anchored to a period, so a lost
//   period is simply a missed hour of archiving; running unguarded to avoid that
//   risks a duplicate proactive DM to a customer, which is unrecoverable.
//
//   A run DEFERS. Same fail-closed instinct, different consequence: a run that
//   cannot take a lease must NOT proceed unleased — the runtime is already
//   at-least-once, and two instances stepping one run concurrently turns that
//   into two side effects at the same instant with interleaved checkpoint
//   writes, which is the one failure mode a checkpoint cannot recover from. But
//   it must not be marked failed either. `research.ts` is the cautionary tale in
//   this tree: it turns "the app restarted mid-run" into
//   `error: "run went stale"`, destroying a run that had lost nothing. The
//   record is durable; the correct response to an infrastructure outage is to
//   leave the row exactly as it is and let a later sweep claim it when Redis is
//   back. Hence `{ kind: 'unavailable' }` rather than a throw: a caller that
//   ignores the distinction gets no lease, and a caller that reads it knows the
//   difference between "someone else has this run" and "I could not ask".
//
// TESTABILITY IS A DESIGN CONSTRAINT (see harness/run.ts). Every edge to the
// outside world — the Redis client, the token source, the heartbeat timer — is
// a field on `LeaseDeps`, defaulted to the real thing and overridable per call,
// so lease.test.ts drives all of this with no Redis server and no real clock.
import { randomUUID } from 'node:crypto'
import { getRedis } from '../db/redis'

/** The slice of a Redis client a lease needs, structurally — so a test can pass
 *  a fake without a Redis server and without mocking the module. */
export interface LeaseRedis {
  set(key: string, value: string, px: 'PX', ttlMs: number, nx: 'NX'): Promise<'OK' | null>
  eval(script: string, numKeys: number, ...args: string[]): Promise<unknown>
  get(key: string): Promise<string | null>
}

/** A running renewal loop. `stop()` is idempotent and must be called on every
 *  exit path from the leased work, including the failing ones. */
export interface LeaseHeartbeat {
  stop: () => void
}

export interface LeaseDeps {
  /** Resolved per call, not captured at module load: `getRedis()` THROWS when
   *  REDIS_URL is unset, and that throw has to land inside the try that turns it
   *  into `unavailable` rather than at import time. */
  redis: () => LeaseRedis
  /** The value written under the key. See `instanceId` for the format. */
  newToken: () => string
  /** The heartbeat's timer. Injected so a test can drive renewals by hand
   *  instead of racing a real interval. Deliberately NOT unref'd: a process
   *  holding a lease has work in flight, and letting the event loop drain out
   *  from under it is how a lease outlives the thing it was protecting. */
  every: (ms: number, fn: () => void) => LeaseHeartbeat
}

/** Identifies this PROCESS in every lease value it writes.
 *
 *  The uuid suffix on each token is what makes a token unique per ATTEMPT; this
 *  prefix is what makes `leaseHolder` able to answer "that is my own lease" —
 *  the scheduler's normal case, where the key it failed to take is the receipt
 *  from its own run earlier this interval. Two processes on one host share a pid
 *  namespace after a restart, so the random half is not decoration. */
export const instanceId = `${process.pid}-${randomUUID().slice(0, 8)}`

const REAL_DEPS: LeaseDeps = {
  redis: getRedis,
  newToken: () => `${instanceId}:${randomUUID()}`,
  every: (ms, fn) => {
    const handle = setInterval(fn, ms)
    return { stop: () => clearInterval(handle) }
  },
}

/** The one key format. `namespace` separates the policies that share this
 *  primitive ('sched' for a named job's period, 'run' for one run row) and `v1`
 *  is there so a future change to what the value means can be made without a
 *  fleet mid-deploy reading two meanings out of one key. */
export const leaseKey = (namespace: string, name: string): string => `talaria:${namespace}:v1:${name}`

/** A claim on a key, held by THIS process. The value is opaque to callers: it is
 *  the compare-and-set operand and the only proof of ownership there is. */
export interface LeaseToken {
  readonly key: string
  readonly value: string
}

export type AcquireResult =
  | { kind: 'acquired'; token: LeaseToken }
  /** Someone holds it — possibly us, from earlier. Ask `leaseHolder` if the
   *  distinction matters; it costs a round trip, which is why it is not folded
   *  in here. */
  | { kind: 'held' }
  /** Redis could not be asked. NOT a failure of the leased work — see the
   *  header. The caller decides whether that means skip (scheduler) or defer
   *  (runs); it never means "mark it broken". */
  | { kind: 'unavailable'; error: unknown }

/** The outcome of an operation on a lease we believe we hold. */
export type LeaseResult =
  | { kind: 'ok' }
  /** The compare-and-set found a different value (or none): the lease expired
   *  and may already belong to someone else. For a job that means the run
   *  overran its TTL; for a run it means another instance has reclaimed the row
   *  and THIS process must stop writing to it. */
  | { kind: 'lost' }
  | { kind: 'unavailable'; error: unknown }

const OK: LeaseResult = { kind: 'ok' }
const LOST: LeaseResult = { kind: 'lost' }

/** Extend (or shorten) a lease we still hold. Compare-and-set: if the value is
 *  no longer ours the lease expired and someone else may have taken it, so we
 *  must not touch it. Returns 1 when the expiry was set, 0 when it was not. */
const CAS_PEXPIRE = `
if redis.call('get', KEYS[1]) == ARGV[1] then
  return redis.call('pexpire', KEYS[1], ARGV[2])
end
return 0
`

/** Give up a lease we still hold, for the same reason and with the same check:
 *  an unconditional DEL from a process whose lease had already expired would
 *  delete the lease another instance is currently working under, and both would
 *  then believe they were alone. */
const CAS_DEL = `
if redis.call('get', KEYS[1]) == ARGV[1] then
  return redis.call('del', KEYS[1])
end
return 0
`

/** Take `key` for `ttlMs`, or report why not. Never throws. */
export async function acquireLease(key: string, ttlMs: number, deps: Partial<LeaseDeps> = {}): Promise<AcquireResult> {
  const d: LeaseDeps = { ...REAL_DEPS, ...deps }
  const token: LeaseToken = { key, value: d.newToken() }
  try {
    const res = await d.redis().set(key, token.value, 'PX', clampTtl(ttlMs), 'NX')
    return res === 'OK' ? { kind: 'acquired', token } : { kind: 'held' }
  } catch (error) {
    return { kind: 'unavailable', error }
  }
}

/** Whether the holder of a lease we failed to take is US.
 *
 *  A separate round trip on purpose. The scheduler wants it for one log line on
 *  a path taken once per interval; a run claim loop can be turned away hundreds
 *  of times a minute by instances legitimately working, and making every one of
 *  those cost a GET would be a real load for an answer nobody reads. Null when
 *  the key is gone or the answer could not be read — this is diagnostics, and it
 *  must never be the reason anything fails. */
export async function leaseHolder(key: string, deps: Partial<LeaseDeps> = {}): Promise<'self' | 'other' | null> {
  const d: LeaseDeps = { ...REAL_DEPS, ...deps }
  try {
    const held = await d.redis().get(key)
    if (!held) return null
    return held.startsWith(`${instanceId}:`) ? 'self' : 'other'
  } catch {
    return null
  }
}

/** Keep a lease we hold alive for another `ttlMs`, from now.
 *
 *  `renewLease` and `demoteLease` are the SAME Redis operation and the intent is
 *  the entire difference between them — which is exactly why they have two
 *  names. Renewing says "I am still working"; demoting says "I am done and this
 *  period is spent". A reader of a call site should not have to work out which
 *  one a bare `pexpire` meant. */
export function renewLease(token: LeaseToken, ttlMs: number, deps: Partial<LeaseDeps> = {}): Promise<LeaseResult> {
  return casPexpire(token, ttlMs, deps)
}

/** Hold a lease PAST the work it protected, for `holdMs`, and stop renewing it.
 *
 *  The scheduler's policy and nothing else's: it is what turns "not at the same
 *  moment" into "once per interval, fleet-wide". A run must not do this — see
 *  the header — because the next step belongs to whichever instance is free. */
export function demoteLease(token: LeaseToken, holdMs: number, deps: Partial<LeaseDeps> = {}): Promise<LeaseResult> {
  return casPexpire(token, holdMs, deps)
}

async function casPexpire(token: LeaseToken, ttlMs: number, deps: Partial<LeaseDeps>): Promise<LeaseResult> {
  const d: LeaseDeps = { ...REAL_DEPS, ...deps }
  try {
    const res = await d.redis().eval(CAS_PEXPIRE, 1, token.key, token.value, String(clampTtl(ttlMs)))
    return res === 1 ? OK : LOST
  } catch (error) {
    return { kind: 'unavailable', error }
  }
}

/** Drop a lease we hold so the next claimant can have it immediately.
 *
 *  `lost` here is not an error to report loudly — it means the lease had already
 *  expired and someone else is holding the key, and deleting it is precisely
 *  what we must not do. */
export async function releaseLease(token: LeaseToken, deps: Partial<LeaseDeps> = {}): Promise<LeaseResult> {
  const d: LeaseDeps = { ...REAL_DEPS, ...deps }
  try {
    const res = await d.redis().eval(CAS_DEL, 1, token.key, token.value)
    return res === 1 ? OK : LOST
  } catch (error) {
    return { kind: 'unavailable', error }
  }
}

export interface HeartbeatOptions {
  /** How often to renew. Defaults to a THIRD of the TTL, so a single failed
   *  renewal — a blip, a failover — is survivable rather than fatal: there are
   *  two more attempts before the lease actually lapses. */
  everyMs?: number
  /** The compare-and-set failed: this process no longer holds the lease and
   *  another instance may already be doing the work. Nothing here stops the work
   *  — the caller owns that decision, and for a run it is a serious one (stop
   *  persisting; the row is not yours any more). */
  onLost?: () => void
  /** Redis could not be reached for a renewal. Distinct from `onLost` because
   *  the lease may well still be ours — we simply could not say so — and the
   *  operator sentence is a different one. */
  onError?: (error: unknown) => void
}

/** Renew a lease in the background while the work it protects runs, so work that
 *  legitimately takes longer than one TTL is not stolen mid-flight. The TTL
 *  stays SHORT and is renewed rather than being set generously up front: the TTL
 *  is also how long a CRASHED holder's lease blocks everyone else, and those two
 *  pressures pull in opposite directions. Renewal is what lets both win. */
export function keepLeaseAlive(token: LeaseToken, ttlMs: number, opts: HeartbeatOptions = {}, deps: Partial<LeaseDeps> = {}): LeaseHeartbeat {
  const d: LeaseDeps = { ...REAL_DEPS, ...deps }
  const everyMs = Math.max(1_000, opts.everyMs ?? Math.floor(clampTtl(ttlMs) / 3))
  return d.every(everyMs, () => {
    // `void` + a total promise: a timer callback that rejects is an unhandled
    // rejection with no stack worth reading, and a failed renewal must never be
    // the thing that takes the process down.
    void renewLease(token, ttlMs, deps).then((r) => {
      if (r.kind === 'lost') opts.onLost?.()
      else if (r.kind === 'unavailable') opts.onError?.(r.error)
    })
  })
}

/** A PX of 0 or less is an error to Redis, and a fractional one is a type
 *  error; both would surface as a lease that could not be taken at all, which
 *  is the least debuggable possible symptom. Clamped here, once, rather than at
 *  four call sites. */
const clampTtl = (ms: number): number => Math.max(1, Math.floor(ms))

// ── The run policy ───────────────────────────────────────────────────────────
//
// A thin, deliberately boring layer over the primitive. It exists so that the
// run runtime never spells out the policy by hand, and so the three things that
// make a RUN lease different from a JOB lease are stated in the API rather than
// remembered: one step's duration, released immediately, and an expiry that
// means reclaim rather than failure.

/** The namespace runs lease under. Separate from 'sched' so a run whose id ever
 *  collided with a job name could not take the job's lease. */
export const RUN_LEASE_NS = 'run'

export const runLeaseKey = (runId: string): string => leaseKey(RUN_LEASE_NS, runId)

export type RunClaim =
  /** This process may step the run. Release it when the step ends — on EVERY
   *  path, including the failing ones — or the run sits idle until the TTL
   *  lapses. */
  | { kind: 'claimed'; lease: LeaseToken }
  /** Another instance is stepping this run right now. Not an error and not a
   *  reason to touch the row: come back later, or move on to another run. */
  | { kind: 'busy' }
  /** Redis could not be asked, so this process cannot know whether it is alone.
   *  LEAVE THE ROW ALONE — do not step it, and above all do not mark it failed.
   *  The checkpoint is durable and a later sweep will claim it. */
  | { kind: 'blocked'; error: unknown }

/** Claim a run for ONE STEP.
 *
 *  `stepMs` is the step's declared outside bound (`RunDefinition.maxStepMs`),
 *  not the run's. That is the whole difference from the scheduler, and it is
 *  what makes a crashed instance cheap: the row becomes claimable again roughly
 *  one step after the process holding it died, rather than one run.
 *
 *  AT-LEAST-ONCE LIVES HERE. This lease stops two instances stepping a run at
 *  the same time; it does NOT stop a step that ran and died before persisting
 *  its checkpoint from running again when someone reclaims the run. Nothing can,
 *  short of the step itself being idempotent — so persist the checkpoint BEFORE
 *  the side effect wherever the ordering allows, and where it does not, say so
 *  at the step. */
export async function acquireRunLease(runId: string, stepMs: number, deps: Partial<LeaseDeps> = {}): Promise<RunClaim> {
  const res = await acquireLease(runLeaseKey(runId), stepMs, deps)
  if (res.kind === 'acquired') return { kind: 'claimed', lease: res.token }
  if (res.kind === 'held') return { kind: 'busy' }
  return { kind: 'blocked', error: res.error }
}

/** Keep a claimed run alive while its step runs. Same heartbeat, and `onLost`
 *  here is the sharpest signal in the whole runtime: another instance has
 *  reclaimed this run from its last checkpoint, so anything this process writes
 *  to the row from now on is a write from a ghost. */
export function keepRunLeaseAlive(claim: LeaseToken, stepMs: number, opts: HeartbeatOptions = {}, deps: Partial<LeaseDeps> = {}): LeaseHeartbeat {
  return keepLeaseAlive(claim, stepMs, opts, deps)
}

/** Hand the run back the moment the step is done, so ANY instance can take the
 *  next one. Deliberately a delete and not a demote: there is no "this period is
 *  spent" for a run, and holding the key would pin the run to this process. */
export function releaseRunLease(claim: LeaseToken, deps: Partial<LeaseDeps> = {}): Promise<LeaseResult> {
  return releaseLease(claim, deps)
}
