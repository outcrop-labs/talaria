// WORK SESSIONS UNDER THE RUNTIME. The port's whole claim is that a session
// survives the process that started it, so the interesting tests are the ones
// that kill a driver mid-session and look at what the next one does.
//
// THE SESSIONS RUN THROUGH THE REAL `drive()`, against an in-memory store and an
// in-memory lease. That matters more here than in run.test.ts: the properties
// under test — "a reclaim re-enters at the persisted checkpoint", "the attempt
// counter tells a step that a driver died holding it", "the turn cap is in the
// checkpoint and not in a closure" — are properties of the DRIVER and the
// definition together, and a test that stepped the definition by hand would be
// asserting against its own loop.
//
// The fake store reimplements store.ts's compare-and-set predicates honestly
// (every write requires `lease_owner = token and state = 'running'`; `claim`
// refuses a live lease and bumps `attempt` only when the previous state was
// `running`), because a reclaim IS that increment and a fake that skipped it
// would make every assertion below vacuous.
import { describe, expect, it } from 'vitest'
import { drive, type RunDeps, type RunLease } from '@/server/runs/run'
import { isTerminal, type AnyRunDefinition, type RunRow, type RunStepContext } from '@/server/runs/define'
import type { NewRun, RunStore, WriteFailure, WriteResult } from '@/server/runs/store'
import {
  MAX_SESSION_TURNS,
  sessionRunId,
  workSessionRun,
  workSessionStep,
  WORK_SESSION_KIND,
  type SessionState,
  type WorkSessionCheckpoint,
  type WorkSessionDeps,
  type WorkSessionInput,
} from '@/server/runs/defs/work-session'
import { dispatchTicketWork, type DispatchDeps } from '@/server/work-dispatch'
import type { Task } from '@/lib/task-const'

// ── The fake world ───────────────────────────────────────────────────────────

class Clock {
  constructor(public t = 1_700_000_000_000) {}
  now = (): number => this.t
  advance(ms: number): void {
    this.t += ms
  }
}

class MemoryStore implements RunStore {
  rows = new Map<string, RunRow>()
  constructor(private clock: Clock) {}

  private iso(t: number): string {
    return new Date(t).toISOString()
  }
  private why(id: string, token: string): WriteFailure {
    const run = this.rows.get(id)
    if (!run) return { ok: false, reason: 'missing' }
    if (run.state === 'cancelled') return { ok: false, reason: 'cancelled' }
    if (run.leaseOwner !== token) return { ok: false, reason: 'lease-lost', state: run.state }
    return { ok: false, reason: 'state', state: run.state }
  }
  /** THE predicate, in one place, exactly as the SQL spells it. */
  private cas(id: string, token: string, mutate: (run: RunRow) => void): WriteResult {
    const run = this.rows.get(id)
    if (!run || run.leaseOwner !== token || run.state !== 'running') return this.why(id, token)
    mutate(run)
    run.updatedAt = this.iso(this.clock.now())
    return { ok: true }
  }

  async insert(row: NewRun): Promise<RunRow> {
    if (this.rows.has(row.id)) throw Object.assign(new Error('duplicate key value violates unique constraint "runs_pkey"'), { code: '23505' })
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
    return { ok: true as const, run: { ...run }, reclaimed }
  }
  async heartbeat({ id, token, leaseMs }: { id: string; token: string; leaseMs: number }): Promise<WriteResult> {
    return this.cas(id, token, (run) => void (run.leaseExpiresAt = this.iso(this.clock.now() + leaseMs)))
  }
  async checkpoint({ id, token, checkpoint, phase, clearDecision }: Parameters<RunStore['checkpoint']>[0]): Promise<WriteResult> {
    return this.cas(id, token, (run) => {
      run.checkpoint = checkpoint
      run.phase = phase
      if (clearDecision) {
        run.decision = null
        run.approvalKey = null
      }
    })
  }
  async phase({ id, token, phase }: Parameters<RunStore['phase']>[0]): Promise<WriteResult> {
    return this.cas(id, token, (run) => void (run.phase = phase))
  }
  async complete({ id, token, result }: Parameters<RunStore['complete']>[0]): Promise<WriteResult> {
    return this.cas(id, token, (run) => {
      run.state = 'done'
      run.result = result
      run.leaseOwner = null
      run.leaseExpiresAt = null
      run.finishedAt = this.iso(this.clock.now())
    })
  }
  async fail({ id, token, error }: Parameters<RunStore['fail']>[0]): Promise<WriteResult> {
    return this.cas(id, token, (run) => {
      run.state = 'error'
      run.error = error
      run.leaseOwner = null
      run.leaseExpiresAt = null
      run.finishedAt = this.iso(this.clock.now())
    })
  }
  async park({ id, token, decision, approvalKey, phase }: Parameters<RunStore['park']>[0]): Promise<WriteResult> {
    return this.cas(id, token, (run) => {
      run.state = 'awaiting'
      run.decision = decision
      run.approvalKey = approvalKey
      run.phase = phase
      run.leaseOwner = null
      run.leaseExpiresAt = null
    })
  }
  async defer({ id, token, until, reason }: Parameters<RunStore['defer']>[0]): Promise<WriteResult> {
    return this.cas(id, token, (run) => {
      run.state = 'queued'
      run.phase = reason
      run.leaseExpiresAt = this.iso(until)
    })
  }
  async release({ id, token }: { id: string; token: string }): Promise<void> {
    const run = this.rows.get(id)
    if (!run || run.leaseOwner !== token || run.state !== 'running') return
    run.leaseOwner = null
    run.leaseExpiresAt = null
  }
  async answer(): Promise<{ ok: false; reason: 'missing' }> {
    return { ok: false, reason: 'missing' }
  }
  async cancel(): Promise<{ ok: false; reason: 'missing' }> {
    return { ok: false, reason: 'missing' }
  }
  async due(): Promise<RunRow[]> {
    return []
  }
  async activeFor(): Promise<RunRow[]> {
    return []
  }
}

const memoryLease = (clock: Clock): RunLease => {
  const held = new Map<string, { token: string; until: number }>()
  let n = 0
  return {
    async acquire(runId, stepMs) {
      const cur = held.get(runId)
      if (cur && cur.until > clock.now()) return { ok: false, reason: 'busy' }
      const token = `lease-${++n}`
      held.set(runId, { token, until: clock.now() + stepMs })
      return { ok: true, token }
    },
    async renew(runId, token, stepMs) {
      const cur = held.get(runId)
      if (!cur || cur.token !== token) return 'lost'
      cur.until = clock.now() + stepMs
      return 'ok'
    },
    async release(runId, token) {
      const cur = held.get(runId)
      if (cur?.token === token) held.delete(runId)
    },
  }
}

// ── The ticket, and the agent on the other end ───────────────────────────────

const TASK_ID = 'task-118'
const AGENT = 'nomad'
const BOARD = 'board-1'

const ticket = (status = 'in_progress'): Task =>
  ({
    id: TASK_ID,
    boardId: BOARD,
    ticketRef: 'PLAT-118',
    title: 'Ledger rows lose their task id on retry',
    description: 'A retried usage write drops taskId.',
    status,
    assignees: [AGENT],
    tags: [],
    attachments: [],
  }) as unknown as Task

interface World {
  /** ONE clock for the store, the driver and the session. A settle interval
   *  measured against `Date.now()` while the row's lease is measured against a
   *  test clock is two different worlds, and the deferral test would sit in the
   *  faster of them for ever. */
  clock: Clock
  /** Every prompt actually sent to a model, in order. */
  sent: string[]
  /** Every line written to the ticket, in order. */
  said: Array<{ actor: string; type: string; description: string }>
  /** The order in which the step asked its two questions, so a test can pin
   *  "authority before money" rather than merely "both happened". */
  calls: string[]
  state: SessionState | null
  reply: (turn: number) => string
}

const world = (over: Partial<World> = {}): World => ({
  clock: new Clock(),
  sent: [],
  said: [],
  calls: [],
  state: { task: ticket(), stop: null },
  reply: (turn) => `did the work on turn ${turn}. Next: keep going.`,
  ...over,
})

const depsFor = (w: World): Partial<WorkSessionDeps> => ({
  sessionState: async () => {
    w.calls.push('authority')
    return w.state
  },
  boardHint: async () => ({ activeKey: 'in_progress', assignedKey: 'assigned' }),
  workflowsForTask: async () => [],
  skillNames: async () => new Set<string>(),
  turn: async ({ prompt }) => {
    w.calls.push('model')
    w.sent.push(prompt)
    return { text: w.reply(w.sent.length), findings: [] }
  },
  logActivity: async (_taskId, actor, type, description) => void w.said.push({ actor, type, description }),
  recentActivity: async () => [...w.said].reverse(),
  now: w.clock.now,
})

const defWith = (deps: Partial<WorkSessionDeps>): AnyRunDefinition =>
  ({
    ...workSessionRun,
    step: (ctx: RunStepContext<unknown, unknown>) => workSessionStep(ctx as RunStepContext<WorkSessionInput, WorkSessionCheckpoint>, deps),
  }) as unknown as AnyRunDefinition

const INPUT: WorkSessionInput = { taskId: TASK_ID, agentModel: AGENT, boardId: BOARD, generation: 0 }

interface Bench {
  clock: Clock
  store: MemoryStore
  deps: Partial<RunDeps>
  runId: string
}

function bench(w: World, opts: { checkpoint?: WorkSessionCheckpoint; attempt?: number; dead?: boolean } = {}): Bench {
  const clock = w.clock
  const store = new MemoryStore(clock)
  const runId = sessionRunId(TASK_ID, AGENT, 0)
  const row: RunRow = {
    id: runId,
    kind: WORK_SESSION_KIND,
    ownerUserId: null,
    subjectType: 'task',
    subjectId: TASK_ID,
    // A DEAD DRIVER, spelled out: `running`, holding a lease that has expired.
    // That is exactly what a crashed instance leaves behind, and it is the one
    // shape `claim` reads as a reclaim — so the run comes back with `attempt`
    // one higher than it went in with.
    state: opts.dead ? 'running' : 'queued',
    phase: 'queued',
    checkpoint: opts.checkpoint ?? null,
    input: INPUT,
    result: null,
    error: null,
    attempt: opts.attempt ?? 0,
    leaseOwner: opts.dead ? 'a-driver-that-died' : null,
    leaseExpiresAt: opts.dead ? new Date(clock.now() - 1_000).toISOString() : null,
    approvalKey: null,
    decision: null,
    createdAt: new Date(clock.now()).toISOString(),
    updatedAt: new Date(clock.now()).toISOString(),
    startedAt: opts.dead ? new Date(clock.now()).toISOString() : null,
    finishedAt: null,
  }
  store.rows.set(runId, row)
  return {
    clock,
    store,
    runId,
    deps: {
      store,
      lease: memoryLease(clock),
      publish: () => {},
      definitionFor: (kind) => (kind === WORK_SESSION_KIND ? defWith(depsFor(w)) : null),
      now: clock.now,
    },
  }
}

const lines = (w: World): string[] => w.said.map((a) => a.description)

// ── A session, start to finish ───────────────────────────────────────────────

describe('a work session as a run', () => {
  it('dispatches, drives turns, and stops when the ticket leaves the working statuses', async () => {
    const w = world()
    const b = bench(w)
    // Turn three moves the ticket to review, which is what a real agent's
    // report_outcome does — the session finds out at its next authority check.
    let turns = 0
    const base = depsFor(w)
    const deps: Partial<WorkSessionDeps> = {
      ...base,
      turn: async (args) => {
        turns++
        if (turns === 3) w.state = { task: ticket('quality_review'), stop: 'ticket moved to "quality_review"' }
        return base.turn!(args)
      },
    }
    b.deps.definitionFor = (kind) => (kind === WORK_SESSION_KIND ? defWith(deps) : null)

    const res = await drive(b.runId, b.deps)
    expect(res.stop).toBe('done')
    expect(w.sent).toHaveLength(3)
    expect(w.sent[0]).toContain('[Assigned work')
    expect(w.sent[1]).toContain('[Work session — turn 2/12]')
    expect(lines(w)).toEqual([
      `work pushed to ${AGENT}`,
      'picked up: did the work on turn 1. Next: keep going.',
      'session turn 2: did the work on turn 2. Next: keep going.',
      'session turn 3: did the work on turn 3. Next: keep going.',
      'work session ended after 3 turns — ticket moved to "quality_review"',
    ])
    expect(b.store.rows.get(b.runId)?.state).toBe('done')
  })

  it('nudges the agent to reconcile when it says DONE and the ticket disagrees', async () => {
    const w = world({ reply: (n) => (n === 1 ? 'made the fix. DONE' : 'moved it to review now. DONE') })
    const b = bench(w)
    let seen = 0
    const base = depsFor(w)
    b.deps.definitionFor = () =>
      defWith({
        ...base,
        turn: async (args) => {
          seen++
          if (seen === 2) w.state = { task: ticket('quality_review'), stop: 'ticket moved to "quality_review"' }
          return base.turn!(args)
        },
      })
    await drive(b.runId, b.deps)
    expect(w.sent[1]).toContain('[Work session — reconcile]')
    expect(lines(w).some((l) => l.startsWith('session reconcile:'))).toBe(true)
  })

  it('refuses before it says anything when the ticket was withdrawn while the run sat in the queue', async () => {
    const w = world({ state: { task: ticket('done'), stop: 'agents cannot change a closed ticket' } })
    const b = bench(w)
    const res = await drive(b.runId, b.deps)
    expect(res.stop).toBe('done')
    // Nothing was pushed, so there is nothing to explain on the ticket — the
    // same silence `maybeDispatchTicket` refuses with.
    expect(w.said).toHaveLength(0)
    expect(w.sent).toHaveLength(0)
  })
})

// ── Resume ───────────────────────────────────────────────────────────────────

describe('a resumed session', () => {
  it('re-checks authority before it sends its next turn', async () => {
    const w = world()
    // A driver died holding turn 2's REPLY, before the activity line for it.
    const b = bench(w, {
      dead: true,
      attempt: 1,
      checkpoint: { stage: 'record', turn: 2, stageAttempt: 1, said: 'turn', reply: { head: 'worked on it', tail: 'worked on it', checks: [] } },
    })
    w.state = { task: ticket(), stop: null }
    let stopAfter = 0
    const base = depsFor(w)
    b.deps.definitionFor = () =>
      defWith({
        ...base,
        turn: async (args) => {
          if (++stopAfter === 1) w.state = { task: ticket('blocked'), stop: 'ticket moved to "blocked"' }
          return base.turn!(args)
        },
      })

    await drive(b.runId, b.deps)
    // THE ORDER IS THE ASSERTION. The first thing a reclaimed session does
    // before spending money is ask whether it is still allowed to.
    expect(w.calls.indexOf('authority')).toBeLessThan(w.calls.indexOf('model'))
    expect(w.calls[0]).toBe('authority')
  })

  it('stops on a withdrawn ticket instead of taking its next turn', async () => {
    const w = world({ state: { task: ticket('blocked'), stop: 'agents cannot work an archived ticket — a person restores it first' } })
    const b = bench(w, {
      dead: true,
      attempt: 1,
      checkpoint: { stage: 'send', turn: 4, stageAttempt: 1, lastTail: 'still going' },
    })
    const res = await drive(b.runId, b.deps)
    expect(res.stop).toBe('done')
    expect(w.sent).toHaveLength(0)
    expect(lines(w)).toEqual(['work session ended after 3 turns — agents cannot work an archived ticket — a person restores it first'])
  })

  it('does NOT re-send a turn that was in flight when its driver died', async () => {
    const w = world()
    // `stageAttempt: 0` and a reclaim takes `attempt` to 1: the step can see
    // that a driver died while this exact turn was owed, so the model may
    // already have it. Re-asking would be a second agent on one ticket.
    const b = bench(w, { dead: true, attempt: 0, checkpoint: { stage: 'send', turn: 5, stageAttempt: 0, lastTail: '' } })
    const res = await drive(b.runId, b.deps)
    expect(w.sent).toHaveLength(0)
    expect(lines(w)).toEqual(['turn 5 was interrupted by a restart — its outcome is unknown; continuing from the ticket\'s current state'])
    // And it waits for whatever may still be running on the far side before it
    // sends turn 6 — a soft pause, so no attempt is consumed and nobody is told.
    expect(res.stop).toBe('deferred')
    expect(b.store.rows.get(b.runId)?.state).toBe('queued')
  })

  it('resumes normally once the settle interval has elapsed', async () => {
    const w = world()
    const b = bench(w, { dead: true, attempt: 0, checkpoint: { stage: 'send', turn: 5, stageAttempt: 0, lastTail: '' } })
    await drive(b.runId, b.deps)
    b.clock.advance(10 * 60_000)
    w.state = { task: ticket(), stop: null }
    const again = await drive(b.runId, b.deps)
    expect(again.stop).toBe('done')
    // It picks up at SIX, not at one: the retired turn still cost a turn.
    expect(w.sent[0]).toContain('[Work session — turn 6/12]')
  })

  it('does not write the ticket line twice when the driver died between the write and the checkpoint', async () => {
    const w = world()
    w.said.push({ actor: AGENT, type: 'dispatch', description: 'session turn 2: worked on it' })
    const b = bench(w, {
      dead: true,
      attempt: 1,
      checkpoint: { stage: 'record', turn: 2, stageAttempt: 1, said: 'turn', reply: { head: 'worked on it', tail: 'worked on it', checks: [] } },
    })
    w.state = { task: ticket('blocked'), stop: 'ticket moved to "blocked"' }
    await drive(b.runId, b.deps)
    expect(lines(w).filter((l) => l === 'session turn 2: worked on it')).toHaveLength(1)
  })
})

// ── The cap ──────────────────────────────────────────────────────────────────

describe('the turn cap', () => {
  it('survives a reclaim — a resumed session does not get a fresh budget', async () => {
    const w = world()
    // Eleven turns are already spent and the eleventh reply is on the
    // checkpoint. A driver died before its activity line landed.
    const b = bench(w, {
      dead: true,
      attempt: 1,
      checkpoint: { stage: 'record', turn: MAX_SESSION_TURNS - 1, stageAttempt: 1, said: 'turn', reply: { head: 'still going', tail: 'still going', checks: [] } },
    })
    const res = await drive(b.runId, b.deps)
    expect(res.stop).toBe('done')
    // ONE turn of budget was left when it was reclaimed, and one turn is what
    // it got — not a fresh twelve, which is what the pre-port re-dispatch gave.
    expect(w.sent).toHaveLength(1)
    expect(w.sent[0]).toContain(`[Work session — turn ${MAX_SESSION_TURNS}/${MAX_SESSION_TURNS}]`)
    expect(lines(w).at(-1)).toBe(`work session hit the ${MAX_SESSION_TURNS}-turn cap — leaving the ticket to the agent/heartbeat`)
  })

  it('counts a turn retired by a reclaim against the budget', async () => {
    const w = world()
    const b = bench(w, {
      dead: true,
      attempt: 1,
      checkpoint: { stage: 'send', turn: MAX_SESSION_TURNS, stageAttempt: 1, lastTail: 'still going' },
    })
    // The last turn was in flight when the driver died, so it is retired rather
    // than re-sent — and retiring it still spends it. No settle wait either:
    // the cap is asked before the wait, because there is nothing to wait for.
    const res = await drive(b.runId, b.deps)
    expect(res.stop).toBe('done')
    expect(w.sent).toHaveLength(0)
    expect(lines(w).at(-1)).toBe(`work session hit the ${MAX_SESSION_TURNS}-turn cap — leaving the ticket to the agent/heartbeat`)
  })

  it('ends at the cap without asking the ticket anything', async () => {
    const w = world()
    const b = bench(w, { checkpoint: { stage: 'send', turn: MAX_SESSION_TURNS + 1, stageAttempt: 0, lastTail: '' } })
    await drive(b.runId, b.deps)
    expect(w.calls).toEqual([])
    expect(lines(w)).toEqual([`work session hit the ${MAX_SESSION_TURNS}-turn cap — leaving the ticket to the agent/heartbeat`])
  })

  it('never sends more turns than the cap, however many drivers die', async () => {
    const w = world()
    const b = bench(w)
    for (let i = 0; i < 30; i++) {
      const res = await drive(b.runId, b.deps)
      if (res.stop === 'done' || res.stop === 'error') break
      // Kill the driver: leave the row `running` with an expired lease, which is
      // all a crashed instance ever leaves behind.
      const row = b.store.rows.get(b.runId)
      if (!row || isTerminal(row.state)) break
      row.state = 'running'
      row.leaseOwner = 'a-driver-that-died'
      row.leaseExpiresAt = new Date(b.clock.now() - 1).toISOString()
      b.clock.advance(10 * 60_000)
    }
    expect(w.sent.length).toBeLessThanOrEqual(MAX_SESSION_TURNS)
    expect(b.store.rows.get(b.runId)?.state).not.toBe('queued')
  })
})

// ── Failure ──────────────────────────────────────────────────────────────────

describe('a turn that produces nothing usable', () => {
  it('tells the ticket and files the run as an error, without re-sending', async () => {
    const w = world()
    const b = bench(w)
    b.deps.definitionFor = () =>
      defWith({
        ...depsFor(w),
        turn: async () => {
          w.calls.push('model')
          throw new Error('gateway 503')
        },
      })
    const res = await drive(b.runId, b.deps)
    expect(res.stop).toBe('error')
    expect(lines(w).at(-1)).toBe(`work session with ${AGENT} failed: gateway 503`)
    expect(w.calls.filter((c) => c === 'model')).toHaveLength(1)
  })
})

// ── The dispatch claim ───────────────────────────────────────────────────────

describe('dispatch', () => {
  const dispatchBench = () => {
    const rows = new Map<string, RunRow>()
    const started: string[] = []
    const deps: Partial<DispatchDeps> = {
      getRun: async (id) => rows.get(id) ?? null,
      startRun: async (id, input) => {
        if (rows.has(id)) throw Object.assign(new Error('duplicate key'), { code: '23505' })
        started.push(id)
        rows.set(id, { id, kind: WORK_SESSION_KIND, state: 'queued', input } as unknown as RunRow)
      },
    }
    return { rows, started, deps }
  }

  it('creates ONE run when the same ticket is dispatched twice', async () => {
    const b = dispatchBench()
    await dispatchTicketWork(ticket(), AGENT, undefined, b.deps)
    await dispatchTicketWork(ticket(), AGENT, undefined, b.deps)
    expect(b.started).toHaveLength(1)
    expect(b.started[0]).toBe(sessionRunId(TASK_ID, AGENT, 0))
  })

  it('creates ONE run when two instances dispatch in the same instant', async () => {
    const b = dispatchBench()
    await Promise.all([dispatchTicketWork(ticket(), AGENT, undefined, b.deps), dispatchTicketWork(ticket(), AGENT, undefined, b.deps)])
    // Whoever loses the insert reads its own duplicate key as "somebody already
    // has this session" — which is exactly what it means.
    expect(b.started).toHaveLength(1)
  })

  it('stands down while a session is PARKED, not only while it is running', async () => {
    const b = dispatchBench()
    await dispatchTicketWork(ticket(), AGENT, undefined, b.deps)
    const row = b.rows.get(sessionRunId(TASK_ID, AGENT, 0))!
    row.state = 'awaiting'
    await dispatchTicketWork(ticket(), AGENT, undefined, b.deps)
    expect(b.started).toHaveLength(1)
  })

  it('starts a NEW session once the last one finished', async () => {
    const b = dispatchBench()
    await dispatchTicketWork(ticket(), AGENT, undefined, b.deps)
    b.rows.get(sessionRunId(TASK_ID, AGENT, 0))!.state = 'done'
    await dispatchTicketWork(ticket(), AGENT, undefined, b.deps)
    expect(b.started).toEqual([sessionRunId(TASK_ID, AGENT, 0), sessionRunId(TASK_ID, AGENT, 1)])
  })

  it('gives each agent on one ticket its own session', async () => {
    const b = dispatchBench()
    await dispatchTicketWork(ticket(), AGENT, undefined, b.deps)
    await dispatchTicketWork(ticket(), 'atlas', undefined, b.deps)
    expect(b.started).toHaveLength(2)
  })

  it('derives a run id that is a uuid, stable, and different per generation', () => {
    const id = sessionRunId(TASK_ID, AGENT, 0)
    expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-8[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
    expect(sessionRunId(TASK_ID, AGENT, 0)).toBe(id)
    expect(sessionRunId(TASK_ID, AGENT, 1)).not.toBe(id)
    expect(sessionRunId(TASK_ID, 'atlas', 0)).not.toBe(id)
  })
})
