import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  acquireLease,
  acquireRunLease,
  demoteLease,
  instanceId,
  keepLeaseAlive,
  leaseHolder,
  leaseKey,
  releaseLease,
  releaseRunLease,
  renewLease,
  runLeaseKey,
  type LeaseDeps,
  type LeaseRedis,
  type LeaseToken,
} from '@/server/runs/lease'
import { registerJob, schedulerStatus, startScheduler, stopScheduler } from '@/server/scheduler'

// WHAT THIS FILE IS FOR
//   The lease is the only thing standing between "one process does this" and
//   two instances archiving the same conversation, DMing the same customer, or
//   stepping the same run at the same instant. Every property it has is a
//   property somebody's data depends on, and none of them is observable from
//   the outside until the day it is violated in production. So they are
//   asserted here, against a fake Redis with a hand-cranked clock — no server,
//   no timers, no sleeps.
//
//   The fake is small on purpose but it is HONEST about the three things the
//   lease actually rests on: NX means "only if absent", PX means the key really
//   goes away on its own, and the two Lua scripts really do compare before they
//   write. A fake that skipped any of those would pass every test below while
//   the real thing deadlocked or double-ran.

interface Entry {
  value: string
  expiresAt: number
}

/** A Redis with a clock you turn by hand. `at` is milliseconds since an
 *  arbitrary zero; nothing expires until you advance it. */
class FakeRedis implements LeaseRedis {
  at = 0
  /** Set to make every command reject, standing in for an unreachable Redis —
   *  a failover, a network partition, or REDIS_URL pointing at nothing. */
  down: Error | null = null
  readonly calls: string[] = []
  private readonly store = new Map<string, Entry>()

  advance(ms: number): void {
    this.at += ms
  }

  private live(key: string): Entry | null {
    const e = this.store.get(key)
    if (!e) return null
    // Lazy expiry, exactly like the real thing from a client's point of view:
    // the key is gone the moment its deadline passes, whether or not anyone
    // asked.
    if (e.expiresAt <= this.at) {
      this.store.delete(key)
      return null
    }
    return e
  }

  async set(key: string, value: string, _px: 'PX', ttlMs: number, _nx: 'NX'): Promise<'OK' | null> {
    if (this.down) throw this.down
    this.calls.push(`set ${key}`)
    if (this.live(key)) return null
    this.store.set(key, { value, expiresAt: this.at + ttlMs })
    return 'OK'
  }

  async get(key: string): Promise<string | null> {
    if (this.down) throw this.down
    this.calls.push(`get ${key}`)
    return this.live(key)?.value ?? null
  }

  async eval(script: string, _numKeys: number, ...args: string[]): Promise<unknown> {
    if (this.down) throw this.down
    const [key, token, ttl] = args
    if (key === undefined || token === undefined) throw new Error('fake redis: eval needs a key and a token')
    const pexpire = script.includes('pexpire')
    this.calls.push(`${pexpire ? 'pexpire' : 'del'} ${key}`)
    const e = this.live(key)
    // THE COMPARE. Everything the token discipline promises comes from this
    // one line being here rather than the operation running unconditionally.
    if (!e || e.value !== token) return 0
    if (pexpire) {
      e.expiresAt = this.at + Number(ttl ?? 0)
      return 1
    }
    this.store.delete(key)
    return 1
  }

  /** Whether a key is currently claimable — i.e. what the next acquirer sees. */
  free(key: string): boolean {
    return this.live(key) === null
  }
}

let redis: FakeRedis
let minted = 0
/** Deps for a call made BY THIS PROCESS. Tokens are still prefixed with the
 *  real `instanceId`, because `leaseHolder`'s self/other answer is derived from
 *  that prefix and a test that faked it would not be testing it. */
let deps: Partial<LeaseDeps>

beforeEach(() => {
  redis = new FakeRedis()
  minted = 0
  deps = { redis: () => redis, newToken: () => `${instanceId}:token-${++minted}` }
})

/** A lease value belonging to some OTHER process — a second instance of the
 *  app, or this same process before a restart. */
const foreign = (key: string): LeaseToken => ({ key, value: 'other-instance-99:token-1' })

const KEY = leaseKey('test', 'job')

describe('acquire', () => {
  it('takes a free key and hands back a token only this process could have', async () => {
    const res = await acquireLease(KEY, 30_000, deps)
    expect(res.kind).toBe('acquired')
    if (res.kind !== 'acquired') return
    expect(res.token.key).toBe(KEY)
    expect(res.token.value.startsWith(`${instanceId}:`)).toBe(true)
  })

  it('refuses a key someone already holds', async () => {
    await acquireLease(KEY, 30_000, deps)
    expect((await acquireLease(KEY, 30_000, deps)).kind).toBe('held')
  })

  it('mints a fresh token per attempt, so a stale one can never renew a lease that has since changed hands', async () => {
    const first = await acquireLease(KEY, 1_000, deps)
    redis.advance(1_001)
    const second = await acquireLease(KEY, 1_000, deps)
    expect(first.kind).toBe('acquired')
    expect(second.kind).toBe('acquired')
    if (first.kind !== 'acquired' || second.kind !== 'acquired') return
    expect(first.token.value).not.toBe(second.token.value)
    // The point of the previous line: the FIRST token must now be powerless.
    expect(await renewLease(first.token, 1_000, deps)).toEqual({ kind: 'lost' })
  })

  it('frees the key when the lease expires, with nobody doing anything', async () => {
    await acquireLease(KEY, 5_000, deps)
    redis.advance(4_999)
    expect((await acquireLease(KEY, 5_000, deps)).kind).toBe('held')
    redis.advance(2)
    expect((await acquireLease(KEY, 5_000, deps)).kind).toBe('acquired')
  })

  it('separates namespaces, so a run id that collides with a job name cannot take the job lease', () => {
    expect(runLeaseKey('comms-decay')).not.toBe(leaseKey('sched', 'comms-decay'))
  })
})

describe('the token discipline', () => {
  it('renews only for the holder, and a renewal really does extend the deadline', async () => {
    const res = await acquireLease(KEY, 5_000, deps)
    if (res.kind !== 'acquired') throw new Error('expected the lease')
    redis.advance(4_000)
    expect(await renewLease(res.token, 5_000, deps)).toEqual({ kind: 'ok' })
    redis.advance(4_000)
    // Without the renewal this key would have lapsed 3s ago.
    expect(redis.free(KEY)).toBe(false)
  })

  it('will not let a foreign token renew — the bug being prevented is a process whose lease expired keeping alive a lease another instance now holds', async () => {
    const res = await acquireLease(KEY, 5_000, deps)
    if (res.kind !== 'acquired') throw new Error('expected the lease')
    expect(await renewLease(foreign(KEY), 60_000, deps)).toEqual({ kind: 'lost' })
    // And the real holder's deadline is untouched by the attempt.
    redis.advance(5_001)
    expect(redis.free(KEY)).toBe(true)
  })

  it('will not let a foreign token release — an unconditional DEL would hand the key away while the true holder is still working', async () => {
    const res = await acquireLease(KEY, 5_000, deps)
    if (res.kind !== 'acquired') throw new Error('expected the lease')
    expect(await releaseLease(foreign(KEY), deps)).toEqual({ kind: 'lost' })
    expect(redis.free(KEY)).toBe(false)
  })

  it('releases for the holder, immediately, and says lost on a second release', async () => {
    const res = await acquireLease(KEY, 30_000, deps)
    if (res.kind !== 'acquired') throw new Error('expected the lease')
    expect(await releaseLease(res.token, deps)).toEqual({ kind: 'ok' })
    expect(redis.free(KEY)).toBe(true)
    expect(await releaseLease(res.token, deps)).toEqual({ kind: 'lost' })
  })

  it('demotes for the holder, shortening the lease into a "this period is spent" marker', async () => {
    const res = await acquireLease(KEY, 30_000, deps)
    if (res.kind !== 'acquired') throw new Error('expected the lease')
    expect(await demoteLease(res.token, 1_000, deps)).toEqual({ kind: 'ok' })
    // Still held — which is the scheduler's whole point, since the work is done
    // and the key is what stops a second instance running the job this period.
    expect(redis.free(KEY)).toBe(false)
    redis.advance(1_001)
    expect(redis.free(KEY)).toBe(true)
  })

  it('reports the holder as self or other, and null when the key is gone', async () => {
    expect(await leaseHolder(KEY, deps)).toBeNull()
    const res = await acquireLease(KEY, 5_000, deps)
    if (res.kind !== 'acquired') throw new Error('expected the lease')
    expect(await leaseHolder(KEY, deps)).toBe('self')
    await releaseLease(res.token, deps)
    await redis.set(KEY, foreign(KEY).value, 'PX', 5_000, 'NX')
    expect(await leaseHolder(KEY, deps)).toBe('other')
  })
})

describe('an unreachable Redis', () => {
  beforeEach(() => {
    redis.down = new Error('connect ECONNREFUSED')
  })

  it('reports unavailable from every operation rather than throwing, so no caller has to wrap one in a try', async () => {
    const token: LeaseToken = { key: KEY, value: 'mine' }
    expect(await acquireLease(KEY, 5_000, deps)).toMatchObject({ kind: 'unavailable' })
    expect(await renewLease(token, 5_000, deps)).toMatchObject({ kind: 'unavailable' })
    expect(await demoteLease(token, 5_000, deps)).toMatchObject({ kind: 'unavailable' })
    expect(await releaseLease(token, deps)).toMatchObject({ kind: 'unavailable' })
  })

  it('never reports unavailable as "held" — the caller has to be able to tell "someone else has it" from "I could not ask"', async () => {
    const res = await acquireLease(KEY, 5_000, deps)
    expect(res.kind).not.toBe('held')
  })

  it('answers null for the holder, because a diagnostic must never be the thing that fails', async () => {
    expect(await leaseHolder(KEY, deps)).toBeNull()
  })

  it('carries the original error, so the operator sees the reason and not just the word', async () => {
    const res = await acquireLease(KEY, 5_000, deps)
    if (res.kind !== 'unavailable') throw new Error('expected unavailable')
    expect(res.error).toBeInstanceOf(Error)
    expect(String(res.error)).toContain('ECONNREFUSED')
  })
})

describe('the heartbeat', () => {
  /** Captures the renewal callback instead of arming a real interval, so the
   *  test drives renewals rather than racing them. */
  const manual = (): { beat: () => Promise<void>; everyMs: number[]; stopped: () => boolean; deps: Partial<LeaseDeps> } => {
    const everyMs: number[] = []
    let fn: (() => void) | null = null
    let stopped = false
    return {
      everyMs,
      stopped: () => stopped,
      beat: async () => {
        fn?.()
        // The callback is void-returning by design (a timer that rejects is an
        // unhandled rejection), so let its promise settle.
        await Promise.resolve()
        await Promise.resolve()
      },
      deps: {
        ...deps,
        every: (ms, f) => {
          everyMs.push(ms)
          fn = f
          return { stop: () => (stopped = true) }
        },
      },
    }
  }

  it('renews three times per TTL, so one blip is survivable rather than fatal', async () => {
    const m = manual()
    const res = await acquireLease(KEY, 9_000, m.deps)
    if (res.kind !== 'acquired') throw new Error('expected the lease')
    keepLeaseAlive(res.token, 9_000, {}, m.deps)
    expect(m.everyMs).toEqual([3_000])
    redis.advance(8_000)
    await m.beat()
    redis.advance(8_000)
    expect(redis.free(KEY)).toBe(false)
  })

  it('reports a lost lease and a Redis outage separately, because they mean different things to whoever is working', async () => {
    const m = manual()
    const res = await acquireLease(KEY, 3_000, m.deps)
    if (res.kind !== 'acquired') throw new Error('expected the lease')
    const lost = vi.fn()
    const failed = vi.fn()
    const beat = keepLeaseAlive(res.token, 3_000, { onLost: lost, onError: failed }, m.deps)

    redis.advance(3_001) // the lease lapsed while the work ran
    await m.beat()
    expect(lost).toHaveBeenCalledTimes(1)
    expect(failed).not.toHaveBeenCalled()

    redis.down = new Error('connect ECONNREFUSED')
    await m.beat()
    expect(failed).toHaveBeenCalledTimes(1)
    expect(lost).toHaveBeenCalledTimes(1)

    beat.stop()
    expect(m.stopped()).toBe(true)
  })
})

describe('the run policy', () => {
  it('claims a run for one step, and another instance is told busy rather than being handed an error', async () => {
    const first = await acquireRunLease('run-1', 30_000, deps)
    expect(first.kind).toBe('claimed')
    expect((await acquireRunLease('run-1', 30_000, deps)).kind).toBe('busy')
  })

  it('releases immediately on completion, so the NEXT step is claimable by any instance', async () => {
    const claim = await acquireRunLease('run-1', 30_000, deps)
    if (claim.kind !== 'claimed') throw new Error('expected the claim')
    expect(await releaseRunLease(claim.lease, deps)).toEqual({ kind: 'ok' })
    // No cooling-off period, unlike the scheduler's demote: a long run must not
    // be pinned to whichever instance happened to start it.
    expect((await acquireRunLease('run-1', 30_000, deps)).kind).toBe('claimed')
  })

  it('treats an expired run lease as a RECLAIM SIGNAL: the row is claimable again with nothing marked failed', async () => {
    const claim = await acquireRunLease('run-1', 10_000, deps)
    expect(claim.kind).toBe('claimed')
    // The instance holding it died here — no release, no demote, no renewal.
    redis.advance(10_001)
    expect((await acquireRunLease('run-1', 10_000, deps)).kind).toBe('claimed')
  })

  it('blocks rather than proceeding unleased when Redis is unreachable, and blocking is distinct from busy', async () => {
    redis.down = new Error('connect ECONNREFUSED')
    const claim = await acquireRunLease('run-1', 10_000, deps)
    // Distinct from 'busy' on purpose: the runner leaves the row exactly as it
    // is either way, but only one of these is worth an operator's attention,
    // and NEITHER is grounds for marking the run failed. research.ts marking a
    // restart as `error: "run went stale"` is the bug this shape exists to
    // make unwritable.
    expect(claim.kind).toBe('blocked')
  })
})

// ── The other caller ─────────────────────────────────────────────────────────
//
// The scheduler keeps its own policy over the same primitive, and the policy is
// the part that would be silently wrong: a job that ran anyway when it could
// not take a lease archives the same conversations twice. This drives the real
// scheduler with no Redis reachable at all — REDIS_URL unset, which is exactly
// what `getRedis()` throws on — and asserts the two halves of that decision.

describe('the scheduler over the same lease', () => {
  const ran: string[] = []
  let redisUrl: string | undefined
  let schedulerFlag: string | undefined
  const quiet: Array<{ mockRestore: () => void }> = []

  beforeAll(async () => {
    redisUrl = process.env.REDIS_URL
    schedulerFlag = process.env.TALARIA_SCHEDULER
    delete process.env.REDIS_URL
    delete process.env.TALARIA_SCHEDULER
    // The boot check names every required job that did not register, which in a
    // unit test is all of them. Loud on a server, noise here.
    for (const level of ['log', 'warn', 'error'] as const) quiet.push(vi.spyOn(console, level).mockImplementation(() => {}))

    // A leased job (the default: it touches shared state) and a perInstance one
    // (its input is an in-memory queue nobody else can see).
    registerJob({ name: 'comms-decay', everyMs: 600_000, firstRunDelayMs: 0, run: async () => (ran.push('comms-decay'), null) })
    registerJob({ name: 'notification-mail', everyMs: 600_000, firstRunDelayMs: 0, perInstance: true, run: async () => (ran.push('notification-mail'), null) })
    startScheduler()
    await new Promise((r) => setTimeout(r, 25))
  })

  afterAll(async () => {
    await stopScheduler(0)
    for (const spy of quiet) spy.mockRestore()
    if (redisUrl === undefined) delete process.env.REDIS_URL
    else process.env.REDIS_URL = redisUrl
    if (schedulerFlag === undefined) delete process.env.TALARIA_SCHEDULER
    else process.env.TALARIA_SCHEDULER = schedulerFlag
  })

  it('FAILS CLOSED: a job that could not take a lease does not run, and the skip is counted rather than swallowed', () => {
    const job = schedulerStatus().find((s) => s.name === 'comms-decay')
    expect(ran).not.toContain('comms-decay')
    expect(job?.runs).toBe(0)
    expect(job?.leaseSkips).toBe(1)
    // A skipped tick is not a FAILURE — nothing is broken and nothing should
    // alert on it. The job simply runs next interval.
    expect(job?.lastError).toBeNull()
  })

  it('still runs a perInstance job with Redis down, because a lease was never protecting it', () => {
    expect(ran).toContain('notification-mail')
    expect(schedulerStatus().find((s) => s.name === 'notification-mail')?.runs).toBe(1)
  })
})

afterEach(() => {
  vi.restoreAllMocks()
})
