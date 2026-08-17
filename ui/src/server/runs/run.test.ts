import { describe, expect, it, vi } from 'vitest'
import { defineRun, type AnyRunDefinition, type DecisionAnswer, type RunDefinition, type RunRow, type RunState } from '@/server/runs/define'
import { cancelRun, drive, enqueue, type LeaseClaim, type LeaseRenewal, type RunDeps, type RunEvent, type RunLease } from '@/server/runs/run'
import { decide } from '@/server/runs/decide'
import { sweepReclaimableRuns } from '@/server/runs/reclaim'
import type { PendingApproval } from '@/server/approvals'
import type { NewRun, RunStore, WriteFailure, WriteResult } from '@/server/runs/store'

// The driver is exercised END TO END against an in-memory store and an
// in-memory lease. Every edge — the store, the lease, the publish, the park,
// the audience resolver, the announcer, the clock, the id generator, the
// definition registry —
// is a field on `RunDeps`, so nothing here touches Postgres or Redis. Same
// pattern and same reason as server/harness/run.test.ts.
//
// THE FAKE STORE IS NOT A STUB. It reimplements the compare-and-set predicates
// from store.ts honestly — every write requires `lease_owner = token and state
// = 'running'`, `claim` refuses a live lease and increments `attempt` only when
// the previous state was `running`. A fake that just wrote whatever it was told
// would turn every assertion below into a restatement of the fake: "a lost
// lease stops cleanly" and "a cancel is honored" are ENTIRELY properties of
// those predicates, and a test that skipped them would pass against a driver
// that had none.

// ── The fake world ───────────────────────────────────────────────────────────

class Clock {
  constructor(public t = 1_700_000_000_000) {}
  now = (): number => this.t
  advance(ms: number): void {
    this.t += ms
  }
}

/** Every persisted write and every publish, in order. The whole ordering rule
 *  — persist, THEN publish — is one assertion over this list. */
type Journal = string[]

const asAny = <I, C>(def: RunDefinition<I, C>): AnyRunDefinition => def as unknown as AnyRunDefinition

class MemoryStore implements RunStore {
  rows = new Map<string, RunRow>()
  constructor(
    private clock: Clock,
    private journal: Journal,
  ) {}

  private iso(t: number): string {
    return new Date(t).toISOString()
  }

  /** The classification store.ts's `why()` does with one extra read. */
  private why(id: string, token: string): WriteFailure {
    const run = this.rows.get(id)
    if (!run) return { ok: false, reason: 'missing' }
    if (run.state === 'cancelled') return { ok: false, reason: 'cancelled' }
    if (run.leaseOwner !== token) return { ok: false, reason: 'lease-lost', state: run.state }
    return { ok: false, reason: 'state', state: run.state }
  }

  /** THE predicate, in one place, exactly as the SQL spells it. */
  private cas(id: string, token: string, mutate: (run: RunRow) => void, label: string): WriteResult {
    const run = this.rows.get(id)
    if (!run || run.leaseOwner !== token || run.state !== 'running') return this.why(id, token)
    mutate(run)
    run.updatedAt = this.iso(this.clock.now())
    this.journal.push(`write:${label}`)
    return { ok: true }
  }

  async insert(row: NewRun): Promise<RunRow> {
    const now = this.iso(this.clock.now())
    const run: RunRow = {
      id: row.id,
      kind: row.kind,
      ownerUserId: row.ownerUserId,
      subjectType: row.subjectType,
      subjectId: row.subjectId,
      state: 'queued',
      phase: row.phase,
      checkpoint: null,
      input: row.input,
      result: null,
      error: null,
      attempt: 0,
      leaseOwner: null,
      leaseExpiresAt: null,
      approvalKey: null,
      decision: null,
      createdAt: now,
      updatedAt: now,
      startedAt: null,
      finishedAt: null,
    }
    this.rows.set(run.id, run)
    this.journal.push('write:insert')
    return { ...run }
  }

  async get(id: string): Promise<RunRow | null> {
    const run = this.rows.get(id)
    return run ? { ...run } : null
  }

  async claim({ id, token, leaseMs }: { id: string; token: string; leaseMs: number }) {
    const run = this.rows.get(id)
    if (!run) return { ok: false as const, reason: 'missing' as const }
    const live = run.leaseExpiresAt !== null && new Date(run.leaseExpiresAt).getTime() > this.clock.now()
    if ((run.state !== 'queued' && run.state !== 'running') || live) {
      if (run.state === 'queued' || run.state === 'running')
        return { ok: false as const, reason: 'taken' as const, state: run.state, until: run.leaseExpiresAt }
      return { ok: false as const, reason: 'not-runnable' as const, state: run.state }
    }
    const reclaimed = run.state === 'running'
    if (reclaimed) run.attempt += 1
    run.state = 'running'
    run.leaseOwner = token
    run.leaseExpiresAt = this.iso(this.clock.now() + leaseMs)
    run.startedAt ??= this.iso(this.clock.now())
    run.updatedAt = this.iso(this.clock.now())
    this.journal.push('write:claim')
    return { ok: true as const, run: { ...run }, reclaimed }
  }

  async heartbeat({ id, token, leaseMs }: { id: string; token: string; leaseMs: number }): Promise<WriteResult> {
    const run = this.rows.get(id)
    if (!run || run.leaseOwner !== token || run.state !== 'running') return this.why(id, token)
    run.leaseExpiresAt = this.iso(this.clock.now() + leaseMs)
    return { ok: true }
  }

  async checkpoint({ id, token, checkpoint, phase, clearDecision }: { id: string; token: string; checkpoint: unknown; phase: string; clearDecision: boolean }): Promise<WriteResult> {
    return this.cas(
      id,
      token,
      (run) => {
        run.checkpoint = checkpoint
        run.phase = phase
        if (clearDecision) {
          run.decision = null
          run.approvalKey = null
        }
      },
      'checkpoint',
    )
  }

  async phase({ id, token, phase }: { id: string; token: string; phase: string }): Promise<WriteResult> {
    return this.cas(id, token, (run) => void (run.phase = phase), 'phase')
  }

  async complete({ id, token, result }: { id: string; token: string; result: unknown }): Promise<WriteResult> {
    return this.cas(
      id,
      token,
      (run) => {
        run.state = 'done'
        run.result = result
        run.error = null
        run.decision = null
        run.approvalKey = null
        run.leaseOwner = null
        run.leaseExpiresAt = null
        run.finishedAt = this.iso(this.clock.now())
      },
      'complete',
    )
  }

  async fail({ id, token, error }: { id: string; token: string; error: string }): Promise<WriteResult> {
    return this.cas(
      id,
      token,
      (run) => {
        run.state = 'error'
        run.error = error
        run.leaseOwner = null
        run.leaseExpiresAt = null
        run.finishedAt = this.iso(this.clock.now())
      },
      'fail',
    )
  }

  async park({ id, token, decision, approvalKey, phase }: Parameters<RunStore['park']>[0]): Promise<WriteResult> {
    return this.cas(
      id,
      token,
      (run) => {
        run.state = 'awaiting'
        run.decision = decision
        run.approvalKey = approvalKey
        run.phase = phase
        run.leaseOwner = null
        run.leaseExpiresAt = null
      },
      'park',
    )
  }

  async defer({ id, token, until, reason }: { id: string; token: string; until: number; reason: string }): Promise<WriteResult> {
    return this.cas(
      id,
      token,
      (run) => {
        run.state = 'queued'
        run.phase = reason
        run.leaseExpiresAt = this.iso(until)
      },
      'defer',
    )
  }

  async release({ id, token }: { id: string; token: string }): Promise<void> {
    const run = this.rows.get(id)
    if (!run || run.leaseOwner !== token || run.state !== 'running') return
    run.leaseOwner = null
    run.leaseExpiresAt = null
  }

  async answer({ id, answer }: { id: string; answer: DecisionAnswer }) {
    const run = this.rows.get(id)
    if (!run) return { ok: false as const, reason: 'missing' as const }
    if (run.state !== 'awaiting') return { ok: false as const, reason: 'not-awaiting' as const, state: run.state }
    if (run.decision?.request.key !== answer.key) return { ok: false as const, reason: 'stale-key' as const, state: run.state }
    run.decision = { request: run.decision.request, answer }
    run.state = 'queued'
    run.leaseOwner = null
    run.leaseExpiresAt = null
    run.updatedAt = this.iso(this.clock.now())
    this.journal.push('write:answer')
    return { ok: true as const, run: { ...run } }
  }

  async cancel({ id, reason }: { id: string; reason?: string }) {
    const run = this.rows.get(id)
    if (!run) return { ok: false as const, reason: 'missing' as const }
    if (run.state !== 'queued' && run.state !== 'running' && run.state !== 'awaiting')
      return { ok: false as const, reason: 'terminal' as const, state: run.state }
    const was = run.state
    run.state = 'cancelled'
    run.error = reason ?? null
    run.leaseOwner = null
    run.leaseExpiresAt = null
    run.finishedAt = this.iso(this.clock.now())
    this.journal.push('write:cancel')
    return { ok: true as const, state: was }
  }

  async due({ limit }: { limit: number }): Promise<RunRow[]> {
    return [...this.rows.values()]
      .filter((r) => (r.state === 'queued' || r.state === 'running') && (r.leaseExpiresAt === null || new Date(r.leaseExpiresAt).getTime() <= this.clock.now()))
      .slice(0, limit)
      .map((r) => ({ ...r }))
  }

  async activeFor({ userId, limit }: { userId: string; limit?: number }): Promise<RunRow[]> {
    return [...this.rows.values()]
      .filter((r) => r.ownerUserId === userId && (r.state === 'queued' || r.state === 'running' || r.state === 'awaiting'))
      .slice(0, limit ?? 50)
      .map((r) => ({ ...r }))
  }
}

class MemoryLease implements RunLease {
  held = new Map<string, string>()
  /** Redis is unreachable: the driver must leave the row alone rather than run
   *  unguarded or — worse — mark it failed. */
  broken = false
  private n = 0
  async acquire(runId: string): Promise<LeaseClaim> {
    if (this.broken) return { ok: false, reason: 'blocked', error: new Error('redis unreachable') }
    if (this.held.has(runId)) return { ok: false, reason: 'busy' }
    const token = `test-instance:${++this.n}`
    this.held.set(runId, token)
    return { ok: true, token }
  }
  async renew(runId: string, token: string): Promise<LeaseRenewal> {
    return this.held.get(runId) === token ? 'ok' : 'lost'
  }
  async release(runId: string, token: string): Promise<void> {
    if (this.held.get(runId) === token) this.held.delete(runId)
  }
}

/** The deps bag as these tests hand it over. `announce` and `approvalFor` are
 *  server/runs/decide.ts's edges rather than the driver's, and they are here
 *  because the driver no longer parks or resumes a run itself: it delegates to
 *  `pause` and a decision comes back through `decide`. One object describes one
 *  world, which is the reason `DecideDeps` extends the driver's bag in the first
 *  place — a test that faked the store for the driver and let the real announcer
 *  reach Postgres for the pause would be testing neither. */
type TestDeps = Partial<RunDeps> & {
  announce?: (approvalKey: string) => Promise<number>
  approvalFor?: (run: RunRow) => PendingApproval | null
}

interface World {
  clock: Clock
  store: MemoryStore
  lease: MemoryLease
  journal: Journal
  events: RunEvent[]
  /** Approval keys the pause path filed, in order. */
  announced: string[]
  deps: TestDeps
}

function world(defs: AnyRunDefinition[]): World {
  const clock = new Clock()
  const journal: Journal = []
  const store = new MemoryStore(clock, journal)
  const lease = new MemoryLease()
  const events: RunEvent[] = []
  const announced: string[] = []
  const byKind = new Map(defs.map((d) => [d.kind, d]))
  let n = 0
  const deps: TestDeps = {
    store,
    lease,
    publish: (event) => {
      journal.push(`publish:${event.state}`)
      events.push(event)
    },
    audienceFor: async () => ({ content: ['u1'], fact: [] }),
    definitionFor: (kind) => byKind.get(kind) ?? null,
    now: clock.now,
    newId: () => `id-${++n}`,
    announce: async (approvalKey) => {
      announced.push(approvalKey)
      return 1
    },
    // The census's row → approval translation, faked to the authority the
    // definition declares. The real one goes through the global run registry,
    // which these locally-defined kinds are deliberately not in.
    approvalFor: (run) => {
      const def = byKind.get(run.kind)
      if (!def || run.state !== 'awaiting' || !run.approvalKey || !run.decision) return null
      return {
        kind: 'run_decision',
        key: run.approvalKey,
        id: run.id,
        title: run.decision.request.question,
        detail: '',
        href: '/',
        waitingSince: run.updatedAt,
        ownerUserIds: run.ownerUserId ? [run.ownerUserId] : [],
        authority: def.audience(run),
      }
    },
  }
  return { clock, store, lease, journal, events, announced, deps }
}

// ── The runs under test ──────────────────────────────────────────────────────

interface CountInput {
  to: number
}
interface CountCheckpoint {
  at: number
}

/** Counts to `input.to`, one step at a time. The `seen` array records the
 *  checkpoint each step was ENTERED with, which is the only way to prove a
 *  resume re-entered from the persisted checkpoint rather than replaying. */
function counter(seen: Array<number | null>) {
  return defineRun<CountInput, CountCheckpoint>({
    kind: 'test-counter',
    label: 'Counter',
    maxStepMs: 30_000,
    audience: () => ({ by: 'admin' }),
    step: async (ctx) => {
      seen.push(ctx.checkpoint?.at ?? null)
      const at = (ctx.checkpoint?.at ?? 0) + 1
      if (at >= ctx.input.to) return { kind: 'done', result: { counted: at } }
      ctx.log(`counted ${at}`)
      return { kind: 'next', checkpoint: { at }, phase: `at ${at}` }
    },
  })
}

describe('runs: enqueue', () => {
  it('writes the row and publishes it, and returns without doing the work', async () => {
    const seen: Array<number | null> = []
    const def = counter(seen)
    const w = world([asAny(def)])

    const run = await enqueue(def, { to: 3 }, { ownerUserId: 'u1', subjectType: 'task', subjectId: 't1', start: false }, w.deps)

    expect(run.id).toBe('id-1')
    expect(run.state).toBe('queued')
    expect(run.kind).toBe('test-counter')
    expect(run.ownerUserId).toBe('u1')
    expect(run.subjectId).toBe('t1')
    expect(run.checkpoint).toBeNull()
    expect(run.attempt).toBe(0)
    // It returned immediately: nothing ran.
    expect(seen).toEqual([])
    // THE ORDERING RULE: the row exists before anybody is told it does.
    expect(w.journal).toEqual(['write:insert', 'publish:queued'])
    expect(w.store.rows.get(run.id)?.input).toEqual({ to: 3 })
  })

  it('starts the run detached by default', async () => {
    const seen: Array<number | null> = []
    const def = counter(seen)
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 2 }, { ownerUserId: 'u1' }, w.deps)
    // The detached drive is a promise nobody awaited; give the microtask queue
    // a turn, which is all an in-memory store needs.
    await new Promise((r) => setTimeout(r, 0))
    expect(w.store.rows.get(run.id)?.state).toBe('done')
  })
})

describe('runs: drive', () => {
  it('runs steps to done, persisting a checkpoint between each', async () => {
    const seen: Array<number | null> = []
    const def = counter(seen)
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 3 }, { ownerUserId: 'u1', start: false }, w.deps)

    const result = await drive(run.id, w.deps)

    expect(result.stop).toBe('done')
    expect(result.steps).toBe(3)
    // Each step saw the checkpoint the previous one persisted — not a replay.
    expect(seen).toEqual([null, 1, 2])
    const row = w.store.rows.get(run.id)!
    expect(row.state).toBe('done')
    expect(row.result).toEqual({ counted: 3 })
    expect(row.leaseOwner).toBeNull()
    expect(row.finishedAt).not.toBeNull()
    // The lease was handed back rather than left to time out.
    expect(w.lease.held.size).toBe(0)
  })

  it('never publishes a state before it has persisted it', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 3 }, { start: false }, w.deps)
    w.journal.length = 0
    await drive(run.id, w.deps)

    // A publish that outran its write is how a second device renders state the
    // database does not have: the user's other tab shows "done", refetches, and
    // gets a run that is still running. It reads as a UI bug for weeks.
    //
    // ASSERTED AS ADJACENCY, not as "some write happened earlier". The weaker
    // form is satisfied by a driver that writes once at the start and then
    // publishes freely for the rest of the run, which is exactly the bug.
    expect(w.journal[0]).toMatch(/^write:/)
    for (const [i, entry] of w.journal.entries()) {
      if (!entry.startsWith('publish:')) continue
      expect({ publish: entry, precededBy: w.journal[i - 1] }).toEqual({ publish: entry, precededBy: expect.stringMatching(/^write:/) })
    }
    expect(w.journal.at(-2)).toBe('write:complete')
    expect(w.journal.at(-1)).toBe('publish:done')
  })

  it('refuses to drive a run nothing on this instance knows how to advance', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 2 }, { start: false }, w.deps)
    const result = await drive(run.id, { ...w.deps, definitionFor: () => null })
    // NOT an error on the row: another instance may well have the code.
    expect(result.stop).toBe('no-definition')
    expect(w.store.rows.get(run.id)?.state).toBe('queued')
  })

  it('leaves the row alone when the lease cannot be reached', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 2 }, { start: false }, w.deps)
    w.lease.broken = true
    const result = await drive(run.id, w.deps)
    // Fails CLOSED: not driven, and — the part that matters — not marked
    // failed either. The checkpoint is durable and a later sweep takes it.
    expect(result.stop).toBe('blocked')
    const row = w.store.rows.get(run.id)!
    expect(row.state).toBe('queued')
    expect(row.error).toBeNull()
  })

  it('reports busy rather than double-running a run another driver holds', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 2 }, { start: false }, w.deps)
    w.lease.held.set(run.id, 'someone-else')
    const result = await drive(run.id, w.deps)
    expect(result.stop).toBe('busy')
    expect(result.steps).toBe(0)
  })
})

/** A driver that VANISHES after `n` checkpoints, leaving the row `running`
 *  with a lease nobody will ever renew — a crashed process, a killed container,
 *  a deploy that took the pod away mid-step. Expressed as another instance
 *  taking the row over, because from this driver's point of view those are the
 *  same event: the next thing it writes is refused. */
function crashAfter(w: World, n: number): Partial<RunDeps> {
  let checkpoints = 0
  return {
    ...w.deps,
    store: new Proxy(w.store, {
      get(target, prop, receiver) {
        if (prop !== 'checkpoint') return Reflect.get(target, prop, receiver)
        return async (args: Parameters<RunStore['checkpoint']>[0]) => {
          const res = await target.checkpoint(args)
          if (++checkpoints === n) {
            const row = target.rows.get(args.id)!
            row.leaseOwner = 'a-driver-that-is-gone'
            row.leaseExpiresAt = new Date(w.clock.now() + 30_000).toISOString()
          }
          return res
        }
      },
    }) as unknown as RunStore,
  }
}

describe('runs: losing the lease', () => {
  it('stops cleanly and leaves the run resumable', async () => {
    const seen: Array<number | null> = []
    const def = counter(seen)
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 5 }, { start: false }, w.deps)

    const result = await drive(run.id, crashAfter(w, 1))

    // A lost lease is a CLEAN STOP, not an error.
    expect(result.stop).toBe('lease-lost')
    expect(result.steps).toBe(1)
    const row = w.store.rows.get(run.id)!
    expect(row.state).toBe('running')
    expect(row.error).toBeNull()
    // And the progress it did make is on the row, so the next driver resumes
    // from it rather than starting over.
    expect(row.checkpoint).toEqual({ at: 1 })
  })

  it('re-driving resumes from the persisted checkpoint and does not replay earlier steps', async () => {
    const seen: Array<number | null> = []
    const def = counter(seen)
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 4 }, { start: false }, w.deps)

    // Two steps' worth of progress, then the driver dies: the row stays
    // `running` with a lease nobody is renewing, which is exactly what a
    // crashed process leaves behind.
    await drive(run.id, crashAfter(w, 2))
    expect(w.store.rows.get(run.id)?.checkpoint).toEqual({ at: 2 })
    expect(w.store.rows.get(run.id)?.state).toBe('running')
    seen.length = 0

    // The lease expires and the sweep finds it.
    w.clock.advance(60_000)
    w.lease.held.clear()
    const result = await drive(run.id, w.deps)

    expect(result.stop).toBe('done')
    // Steps 1 and 2 did NOT run again: the resumed run re-entered `step()` with
    // the persisted checkpoint, which is the whole reason a run is a sequence
    // of steps over one.
    expect(seen).toEqual([2, 3])
    expect(w.store.rows.get(run.id)?.result).toEqual({ counted: 4 })
    // Reclaimed once, so exactly one attempt was consumed.
    expect(w.store.rows.get(run.id)?.attempt).toBe(1)
  })
})

// ── THE RESUME PROPERTY ──────────────────────────────────────────────────────
//
// This is one of the two things the whole runs runtime exists to make true, so
// it gets a test of its own that asserts it end to end rather than in pieces.
// The test above ('re-driving resumes…') kills a driver BETWEEN steps, at a
// checkpoint boundary, which is the easy half. This one kills it in the hardest
// place there is: WHILE A STEP IS RUNNING and after that step has done work it
// has not persisted. That is the shape of every real incident — a deploy, an
// OOM kill, a container paused past its lease — and it is the shape that
// research.ts answers by marking the user's run failed.
//
// It also pins the COST of getting it right, in the same assertion: the step
// that was in flight when the driver died is entered a SECOND time with the
// same checkpoint. That is the at-least-once contract, and it is not a defect
// to be fixed later — it is the guarantee, and the reason every ported step
// owes its side effects a guard.
describe('RESUME: a driver that dies mid-step', () => {
  it('loses its lease while a step is running, and a second driver finishes from the persisted checkpoint without replaying completed steps', async () => {
    /** Every entry into `step`, recorded as the checkpoint it was ENTERED with.
     *  The only honest way to tell a resume from a replay: a replay starts at
     *  `null` again, a resume starts at the last persisted checkpoint. */
    const entries: Array<number | null> = []
    /** Boxed so TypeScript does not narrow it to `null`: the only assignment is
     *  inside the step's own promise executor, which control-flow analysis
     *  cannot see running. */
    const hang: { release: (() => void) | null } = { release: null }
    let hangOnce = true

    const def = defineRun<{ to: number }, { at: number }>({
      kind: 'test-resume-midstep',
      label: 'Dies mid-step',
      maxStepMs: 60_000,
      audience: () => ({ by: 'admin' }),
      step: async (ctx) => {
        entries.push(ctx.checkpoint?.at ?? null)
        // A RUNAWAY BRAKE, and it is here so this test can be mutation-checked
        // rather than as belt and braces. Every interesting way of breaking
        // resume — handing the step a null checkpoint, dropping the checkpoint
        // write, replaying a reclaimed run from zero — turns a counter that
        // counts UP into one that counts to one forever. Without a bound the
        // broken driver spins until the process runs out of heap, which is a
        // test that "fails" only by taking the suite down with it; with one, a
        // replay is a short list that does not match the expected one.
        if (entries.length > 8) return { kind: 'done', result: { ranAway: entries.length } }
        const at = (ctx.checkpoint?.at ?? 0) + 1
        if (at === 3 && hangOnce) {
          // The step is mid-flight — it has started its work and has NOT
          // returned a checkpoint. This is the window the whole design is about.
          hangOnce = false
          await new Promise<void>((resolve) => {
            hang.release = resolve
          })
        }
        if (at >= ctx.input.to) return { kind: 'done', result: { counted: at } }
        return { kind: 'next', checkpoint: { at }, phase: `at ${at}` }
      },
    })

    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 4 }, { ownerUserId: 'u1', start: false }, w.deps)

    // Fake timers so the driver's renewal beat can be fired on demand: its
    // cadence is a third of the lease TTL, and a test that waited that out in
    // real time would be a twenty-second test of a one-line property.
    vi.useFakeTimers()
    try {
      const driverA = drive(run.id, w.deps)
      // Let steps 1 and 2 land and step 3 begin. Everything up to the hang is
      // microtasks against an in-memory store.
      await vi.advanceTimersByTimeAsync(0)
      expect(w.store.rows.get(run.id)?.checkpoint).toEqual({ at: 2 })

      // ANOTHER INSTANCE TAKES THE RUN. From this driver's point of view a
      // crash and a reclaim are the same event — the next thing it tries to
      // write is refused — and this is the version that can be observed.
      w.lease.held.set(run.id, 'another-instance')
      w.store.rows.get(run.id)!.leaseOwner = 'another-instance'

      // The renewal beat notices and aborts the step in flight.
      await vi.advanceTimersByTimeAsync(21_000)
      const a = await driverA

      // A LOST LEASE IS A CLEAN STOP. Not an error row, not a notification, and
      // above all not 'run went stale'.
      expect(a.stop).toBe('lease-lost')
      const parked = w.store.rows.get(run.id)!
      expect(parked.state).toBe('running')
      expect(parked.error).toBeNull()
      expect(parked.checkpoint).toEqual({ at: 2 })
    } finally {
      vi.useRealTimers()
    }

    // The instance that took it dies too, its lease lapses, and the sweep finds
    // the row exactly as the first driver left it.
    w.lease.held.delete(run.id)
    w.clock.advance(120_000)

    const b = await drive(run.id, w.deps)

    expect(b.stop).toBe('done')
    const done = w.store.rows.get(run.id)!
    expect(done.state).toBe('done')
    expect(done.result).toEqual({ counted: 4 })
    // Reclaimed once — the crash cost exactly one attempt, and a healthy run
    // that simply takes many steps costs none.
    expect(done.attempt).toBe(1)

    // THE WHOLE PROPERTY, in one line. Steps 1 and 2 ran ONCE: the second
    // driver re-entered at the persisted checkpoint instead of replaying from
    // zero. And the step that was in flight when the lease was lost — the one
    // that had not persisted — was entered AGAIN with the same checkpoint,
    // which is the at-least-once contract stated as data.
    expect(entries).toEqual([null, 1, 2, 2, 3])

    hang.release?.()
  })
})

describe('runs: a thrown step', () => {
  it('becomes an error state with the message on the row', async () => {
    const def = defineRun<{ n: number }, { at: number }>({
      kind: 'test-throws',
      label: 'Throws',
      maxStepMs: 30_000,
      audience: () => ({ by: 'admin' }),
      step: async () => {
        throw new Error('the upstream said no')
      },
    })
    const w = world([asAny(def)])
    const run = await enqueue(def, { n: 1 }, { start: false }, w.deps)

    const result = await drive(run.id, w.deps)

    // NOT swallowed into a silent failure: "a return null nobody hears about"
    // is the disease, and a run stuck at `running` forever is its other face.
    expect(result.stop).toBe('error')
    expect(result.error).toBe('the upstream said no')
    const row = w.store.rows.get(run.id)!
    expect(row.state).toBe('error')
    expect(row.error).toBe('the upstream said no')
    expect(row.finishedAt).not.toBeNull()
    expect(row.leaseOwner).toBeNull()
    expect(w.events.at(-1)?.state).toBe('error')
  })

  it('files a step that blows its own declared budget, with the budget in the message', async () => {
    const def = defineRun<Record<string, never>, { at: number }>({
      kind: 'test-hangs',
      label: 'Hangs',
      maxStepMs: 10,
      audience: () => ({ by: 'admin' }),
      step: async () => new Promise<never>(() => {}),
    })
    const w = world([asAny(def)])
    const run = await enqueue(def, {}, { start: false }, w.deps)
    const result = await drive(run.id, w.deps)
    expect(result.stop).toBe('error')
    expect(result.error).toContain('maxStepMs')
    expect(w.store.rows.get(run.id)?.state).toBe('error')
  })
})

describe('runs: cancellation', () => {
  it('is honored at the next step boundary, whichever instance issued it', async () => {
    const seen: Array<number | null> = []
    const def = counter(seen)
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 10 }, { ownerUserId: 'u1', start: false }, w.deps)

    // "Another instance" cancels it: a plain row write, with no lease and no
    // knowledge of who is driving. This is the thing fitness/surface.ts's
    // module-level stop flag cannot do.
    const cancelling: Partial<RunDeps> = {
      ...w.deps,
      store: new Proxy(w.store, {
        get(target, prop, receiver) {
          if (prop !== 'checkpoint') return Reflect.get(target, prop, receiver)
          return async (args: Parameters<RunStore['checkpoint']>[0]) => {
            const res = await target.checkpoint(args)
            if (seen.length === 2) await target.cancel({ id: args.id, reason: 'user pressed stop' })
            return res
          }
        },
      }) as unknown as RunStore,
    }

    const result = await drive(run.id, cancelling)

    expect(result.stop).toBe('cancelled')
    // Two steps ran, and the third was never entered.
    expect(seen).toHaveLength(2)
    const row = w.store.rows.get(run.id)!
    expect(row.state).toBe('cancelled')
    expect(row.checkpoint).toEqual({ at: 2 })
  })

  it('refuses a write from the driver that still thinks it owns the run', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 3 }, { start: false }, w.deps)
    // Cancelled while the step was in flight: the compare-and-set on
    // `state = 'running'` rejects the checkpoint that comes back.
    const racing: Partial<RunDeps> = {
      ...w.deps,
      definitionFor: () =>
        asAny(
          defineRun<CountInput, CountCheckpoint>({
            ...counter([]),
            step: async (ctx) => {
              await cancelRun({ runId: ctx.run.id, reason: 'stop' }, w.deps)
              return { kind: 'next', checkpoint: { at: 1 } }
            },
          }),
        ),
    }
    const result = await drive(run.id, racing)
    expect(result.stop).toBe('cancelled')
    expect(w.store.rows.get(run.id)?.checkpoint).toBeNull()
  })

  it('cannot cancel a run that has already finished', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 1 }, { start: false }, w.deps)
    await drive(run.id, w.deps)
    const res = await cancelRun({ runId: run.id }, w.deps)
    expect(res).toEqual({ ok: false, reason: 'terminal', state: 'done' })
  })
})

describe('runs: maxAttempts', () => {
  it('gives up once a run has killed more drivers than it is allowed', async () => {
    const seen: Array<number | null> = []
    const base = counter(seen)
    const def: RunDefinition<CountInput, CountCheckpoint> = { ...base, kind: 'test-attempts', maxAttempts: 2 }
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 99 }, { start: false }, w.deps)

    // Two drivers take it and die without finishing.
    for (let i = 0; i < 2; i++) {
      await drive(run.id, crashAfter(w, 1))
      w.clock.advance(60_000)
      w.lease.held.clear()
    }
    expect(w.store.rows.get(run.id)?.attempt).toBe(1)
    seen.length = 0

    const result = await drive(run.id, w.deps)

    // The third entry takes `attempt` to 2, which is the limit: filed as an
    // error, with the count in the message, and the step is never entered.
    expect(result.stop).toBe('exhausted')
    expect(seen).toEqual([])
    const row = w.store.rows.get(run.id)!
    expect(row.state).toBe('error')
    expect(row.attempt).toBe(2)
    expect(row.error).toContain('gave up after 2 attempt')
  })

  it('does not consume an attempt for a long healthy run', async () => {
    const seen: Array<number | null> = []
    const def: RunDefinition<CountInput, CountCheckpoint> = { ...counter(seen), kind: 'test-long', maxAttempts: 2 }
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 50 }, { start: false }, w.deps)
    const result = await drive(run.id, w.deps)
    // Fifty steps, one entry. A counter that moved per step would have failed
    // this run at step three.
    expect(result.stop).toBe('done')
    expect(result.steps).toBe(50)
    expect(w.store.rows.get(run.id)?.attempt).toBe(0)
  })
})

describe('runs: pausing for a person', () => {
  const askThenAct = (log: string[]) =>
    defineRun<{ subject: string }, { asked: boolean }>({
      kind: 'test-decide',
      label: 'Asks',
      maxStepMs: 30_000,
      audience: (run) => ({ by: 'user', userIds: [run.ownerUserId ?? ''] }),
      step: async (ctx) => {
        if (!ctx.decision)
          return {
            kind: 'decide',
            question: {
              key: 'send-it',
              question: `Send the ${ctx.input.subject}?`,
              options: [
                { id: 'send', label: 'Send' },
                { id: 'drop', label: 'Discard' },
              ],
            },
          }
        log.push(`acted on ${ctx.decision.optionId} by ${ctx.decision.answeredBy}`)
        return { kind: 'done', result: { did: ctx.decision.optionId } }
      },
    })

  it('parks the run, files an approval key, and tells the audience', async () => {
    const log: string[] = []
    const def = askThenAct(log)
    const w = world([asAny(def)])
    const run = await enqueue(def, { subject: 'invoice' }, { ownerUserId: 'u1', start: false }, w.deps)

    const result = await drive(run.id, w.deps)

    expect(result.stop).toBe('awaiting')
    const row = w.store.rows.get(run.id)!
    expect(row.state).toBe('awaiting')
    // The question is ON THE ROW: a decision that lived only in the process
    // that raised it would be gone the moment you opened it on your phone.
    expect(row.decision?.request.question).toBe('Send the invoice?')
    expect(row.decision?.answer).toBeNull()
    // Derived from the run and the question key, so a re-ask after a reclaim
    // dedupes instead of paging somebody twice.
    expect(row.approvalKey).toBe(`run:test-decide:${run.id}:send-it`)
    expect(row.leaseOwner).toBeNull()
    // Filed as an approval under that key — the driver delegates the whole
    // transition to `pause`, so this is the real one running.
    expect(w.announced).toEqual([row.approvalKey])
    // The row was `awaiting` before anybody was told.
    expect(w.journal.indexOf('write:park')).toBeLessThan(w.journal.indexOf('publish:awaiting'))
    expect(log).toEqual([])
  })

  it('will not drive a parked run — driving it would re-ask the question', async () => {
    const def = askThenAct([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { subject: 'invoice' }, { ownerUserId: 'u1', start: false }, w.deps)
    await drive(run.id, w.deps)
    const again = await drive(run.id, w.deps)
    expect(again.stop).toBe('not-runnable')
    expect(again.state).toBe('awaiting')
    expect(w.announced).toHaveLength(1)
  })

  it('resumes with the answer once a person gives one, and clears it after', async () => {
    const log: string[] = []
    const def = askThenAct(log)
    const w = world([asAny(def)])
    const run = await enqueue(def, { subject: 'invoice' }, { ownerUserId: 'u1', start: false }, w.deps)
    await drive(run.id, w.deps)

    // Through `decide`, which is the only door into the `awaiting → queued`
    // write: it resolves the definition's authority and asks `mayDecide` before
    // anything is written. `u1` is who this run's audience resolves to.
    const answered = await decide({ runId: run.id, optionId: 'send', by: 'u1', start: false }, w.deps)
    expect(answered.ok).toBe(true)
    expect(w.store.rows.get(run.id)?.state).toBe('queued')

    const result = await drive(run.id, w.deps)
    expect(result.stop).toBe('done')
    expect(log).toEqual(['acted on send by u1'])
    expect(w.store.rows.get(run.id)?.result).toEqual({ did: 'send' })
  })

  it('persists the park and the resume before it publishes either', async () => {
    // The ordering rule over the two transitions that cross a MODULE boundary —
    // the park lives in decide.ts's `pause` and the resume in its private
    // `resumeAnswered`, and a rule that only holds inside the driver's own loop
    // is a rule that a second file gets to break quietly.
    const def = askThenAct([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { subject: 'invoice' }, { ownerUserId: 'u1', start: false }, w.deps)
    w.journal.length = 0

    await drive(run.id, w.deps)
    await decide({ runId: run.id, optionId: 'send', by: 'u1', start: false }, w.deps)
    await drive(run.id, w.deps)

    for (const [i, entry] of w.journal.entries()) {
      if (!entry.startsWith('publish:')) continue
      expect({ publish: entry, precededBy: w.journal[i - 1] }).toEqual({ publish: entry, precededBy: expect.stringMatching(/^write:/) })
    }
    expect(w.journal).toContain('write:park')
    expect(w.journal).toContain('write:answer')
    expect(w.journal.indexOf('write:park')).toBeLessThan(w.journal.indexOf('publish:awaiting'))
    expect(w.journal.indexOf('write:answer')).toBeLessThan(w.journal.indexOf('publish:queued'))
  })

  it('refuses an answer aimed at a question the run is no longer parked on', async () => {
    // The stale-tab case, expressed the way it can actually arrive now that
    // `decide` names the question from the ROW rather than from the request: a
    // tab still showing last week's options sends an id this question never
    // offered, and it is refused before anything is written.
    const def = askThenAct([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { subject: 'invoice' }, { ownerUserId: 'u1', start: false }, w.deps)
    await drive(run.id, w.deps)

    const stale = await decide({ runId: run.id, optionId: 'approve-the-other-thing', by: 'u1', start: false }, w.deps)

    expect(stale).toEqual({ ok: false, reason: 'unknown-option', state: 'awaiting' })
    expect(w.store.rows.get(run.id)?.decision?.answer).toBeNull()
    expect(w.store.rows.get(run.id)?.state).toBe('awaiting')
  })

  it('clears a consumed answer in the same write that records the progress it produced', async () => {
    // A step that checkpoints after acting on the decision must not be handed
    // the same answer again on the next step: it would act on it twice.
    const acted: string[] = []
    const def = defineRun<Record<string, never>, { done: boolean }>({
      kind: 'test-decide-then-continue',
      label: 'Asks then continues',
      maxStepMs: 30_000,
      audience: () => ({ by: 'admin' }),
      step: async (ctx) => {
        if (ctx.checkpoint?.done) return { kind: 'done', result: { acted: acted.length } }
        if (!ctx.decision) return { kind: 'decide', question: { key: 'q', question: 'Go?', options: [{ id: 'yes', label: 'Yes' }] } }
        acted.push(ctx.decision.optionId)
        return { kind: 'next', checkpoint: { done: true } }
      },
    })
    const w = world([asAny(def)])
    const run = await enqueue(def, {}, { start: false }, w.deps)
    await drive(run.id, w.deps)
    await decide({ runId: run.id, optionId: 'yes', by: 'u1', start: false }, w.deps)
    const result = await drive(run.id, w.deps)

    expect(result.stop).toBe('done')
    expect(acted).toEqual(['yes'])
    const row = w.store.rows.get(run.id)!
    expect(row.decision).toBeNull()
    expect(row.approvalKey).toBeNull()
  })
})

describe('runs: a soft pause', () => {
  it('schedules, bothers nobody, and consumes no attempt', async () => {
    let calls = 0
    const def = defineRun<Record<string, never>, { tries: number }>({
      kind: 'test-retry',
      label: 'Backs off',
      maxStepMs: 30_000,
      maxAttempts: 2,
      audience: () => ({ by: 'admin' }),
      step: async (ctx) => {
        calls++
        if (calls === 1) return { kind: 'retry', after: 30_000, reason: 'rate limited by the provider' }
        return { kind: 'done', result: { tries: (ctx.checkpoint?.tries ?? 0) + 1 } }
      },
    })
    const w = world([asAny(def)])
    const run = await enqueue(def, {}, { start: false }, w.deps)

    const first = await drive(run.id, w.deps)
    expect(first.stop).toBe('deferred')
    expect(first.retryAfterMs).toBe(30_000)
    const parked = w.store.rows.get(run.id)!
    expect(parked.state).toBe('queued')
    expect(parked.attempt).toBe(0)
    expect(parked.phase).toBe('rate limited by the provider')
    // Nobody was told: a soft pause is not a decision.
    expect(w.announced).toEqual([])
    // And nothing may take it yet — the wait IS the unexpired lease.
    expect(await w.store.due({ limit: 10 })).toEqual([])
    expect(w.lease.held.get(run.id)).toBeDefined()

    // The wait elapses and the sweep finds it.
    w.clock.advance(30_001)
    w.lease.held.clear()
    expect((await w.store.due({ limit: 10 })).map((r) => r.id)).toEqual([run.id])
    const second = await drive(run.id, w.deps)
    expect(second.stop).toBe('done')
    // A deferral is not a crash: it did not cost an attempt, so `maxAttempts: 2`
    // is still untouched.
    expect(w.store.rows.get(run.id)?.attempt).toBe(0)
  })
})

// The sweeper's own unit tests are in reclaim.test.ts, against an injected
// `drive`. These two run it against the REAL driver, the real lease and the real
// store predicates, because the property they are about is the seam between
// them — what the sweep hands over actually gets driven, and what it declines to
// hand over stays exactly as it was.
const sweepWith = (w: World) => ({
  due: (args: { limit: number }) => w.store.due(args),
  definitionFor: (kind: string) => w.deps.definitionFor?.(kind) ?? null,
  drive: (runId: string) => drive(runId, w.deps),
  now: w.clock.now,
})

describe('runs: the reclaim sweep', () => {
  it('picks up runs whose driver stopped renewing, and skips kinds it does not know', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const mine = await enqueue(def, { to: 2 }, { start: false }, w.deps)
    const foreign = await enqueue({ ...def, kind: 'from-a-newer-deploy' }, { to: 2 }, { start: false }, w.deps)

    const swept = await sweepReclaimableRuns({ limit: 10 }, sweepWith(w))

    expect(swept.scanned).toBe(2)
    expect(swept.driven).toBe(1)
    // A row this instance has no code for is LEFT ALONE rather than failed: an
    // instance that cannot drive a run is not the same as a run that cannot be
    // driven.
    expect(swept.unknownKinds).toBe(1)
    await new Promise((r) => setTimeout(r, 0))
    expect(w.store.rows.get(mine.id)?.state).toBe('done')
    expect(w.store.rows.get(foreign.id)?.state).toBe('queued')
  })

  it('leaves a finished run alone', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 1 }, { start: false }, w.deps)
    await drive(run.id, w.deps)
    const swept = await sweepReclaimableRuns({ limit: 10 }, sweepWith(w))
    expect(swept.scanned).toBe(0)
    expect(w.store.rows.get(run.id)?.state).toBe('done')
  })
})

describe('runs: progress lines', () => {
  it('persists a phase before it publishes it', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    const run = await enqueue(def, { to: 3 }, { start: false }, w.deps)
    w.journal.length = 0
    await drive(run.id, w.deps)

    const phaseWrite = w.journal.indexOf('write:phase')
    expect(phaseWrite).toBeGreaterThan(-1)
    const publishAfter = w.journal.findIndex((e, i) => i > phaseWrite && e.startsWith('publish:'))
    expect(publishAfter).toBeGreaterThan(phaseWrite)
    // The last phase the run recorded is the one the last step wrote.
    expect(w.store.rows.get(run.id)?.phase).toBe('at 2')
  })
})

describe('runs: the user surface', () => {
  it('lists what a person has in flight, including what is waiting on them', async () => {
    const def = counter([])
    const w = world([asAny(def)])
    await enqueue(def, { to: 5 }, { ownerUserId: 'u1', start: false }, w.deps)
    await enqueue(def, { to: 5 }, { ownerUserId: 'u2', start: false }, w.deps)
    const done = await enqueue(def, { to: 1 }, { ownerUserId: 'u1', start: false }, w.deps)
    await drive(done.id, w.deps)

    const active = await w.store.activeFor({ userId: 'u1' })
    expect(active.map((r) => r.state)).toEqual(['queued'])
  })
})

/** The states are the contract; a typo in one of them is a run nothing drives. */
describe('runs: states', () => {
  it('names exactly six', () => {
    const all: RunState[] = ['queued', 'running', 'awaiting', 'done', 'error', 'cancelled']
    expect(new Set(all).size).toBe(6)
  })
})
