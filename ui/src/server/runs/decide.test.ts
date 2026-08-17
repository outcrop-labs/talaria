// PAUSE AS APPROVAL, end to end: a run parks on a question, the people its
// definition names are told, somebody who is not one of them is refused, the
// person who is answers, and the run goes back to work carrying the answer.
//
// The store and the lease are in-memory, as in run.test.ts, and they REIMPLEMENT
// the compare-and-set predicates rather than accepting whatever they are told —
// "a paused run cannot be driven" and "a decision cannot come from a stranger"
// are properties of those predicates and of the authority check, and a fake that
// wrote whatever it was handed would turn every assertion below into a
// restatement of the fake.
//
// The database edge is faked at `db()` instead: the sweep is the one thing here
// that is NOT injectable, because `sweepUnannounced` is server/approvals.ts's own
// safety net and the point of testing it is that a run_decision falls into it
// exactly like the other four kinds, through the real code.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DecisionAnswer, DecisionRequest, RunRow, RunState } from '@/server/runs/define'
import { defineRun, registerRun } from '@/server/runs/define'
import type { ClaimResult, NewRun, RunStore, WriteFailure, WriteResult } from '@/server/runs/store'
import type { LeaseClaim, LeaseRenewal, RunDeps, RunEvent, RunLease } from '@/server/runs/run'

// ── The database edge, for the sweep only ────────────────────────────────────

const statements: string[] = []
const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
  statements.push(strings.join(' ').replace(/\s+/g, ' ').trim())
  void values
  // Every read the sweep makes is a settings read, and an empty result means
  // "nothing announced yet" — which is the state this test is about.
  return Promise.resolve([])
}) as unknown as {
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]>
  json: (v: unknown) => unknown
  unsafe: (text: string) => string
}
sql.json = (v: unknown) => v
sql.unsafe = (text: string) => text

const notifications: Array<{ userId: string; kind: string; title: string; body: string; href?: string }> = []

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/notifications', () => ({
  addNotification: async (userId: string, n: { kind: string; title: string; body: string; href?: string }) => {
    notifications.push({ userId, ...n })
  },
}))

const { runDecisionApproval, sweepUnannounced } = await import('@/server/approvals')
const { decide, pause, runApprovalKey } = await import('@/server/runs/decide')
const { drive } = await import('@/server/runs/run')

// ── The fake world ───────────────────────────────────────────────────────────

const NOW = 1_700_000_000_000

/** The same compare-and-set predicates store.ts spells in SQL. Narrower than
 *  run.test.ts's copy — only the writes these two halves reach — but not looser:
 *  every write still requires `lease_owner = token and state = 'running'`, which
 *  is the only reason a park cannot be raced and a paused run cannot be driven. */
class MemoryStore implements RunStore {
  rows = new Map<string, RunRow>()
  constructor(private t = NOW) {}

  private iso(at = this.t): string {
    return new Date(at).toISOString()
  }

  private why(id: string, token: string): WriteFailure {
    const run = this.rows.get(id)
    if (!run) return { ok: false, reason: 'missing' }
    if (run.state === 'cancelled') return { ok: false, reason: 'cancelled' }
    if (run.leaseOwner !== token) return { ok: false, reason: 'lease-lost', state: run.state }
    return { ok: false, reason: 'state', state: run.state }
  }

  private cas(id: string, token: string, mutate: (run: RunRow) => void): WriteResult {
    const run = this.rows.get(id)
    if (!run || run.leaseOwner !== token || run.state !== 'running') return this.why(id, token)
    mutate(run)
    run.updatedAt = this.iso()
    return { ok: true }
  }

  /** A run already claimed and stepping, which is the only state a pause can
   *  legally happen from. */
  running(over: Partial<RunRow> = {}): RunRow {
    const run: RunRow = {
      id: 'run-1',
      kind: 'test-decision-run',
      ownerUserId: 'u-owner',
      subjectType: 'task',
      subjectId: 'board-1',
      state: 'running',
      phase: 'picking an assignee',
      checkpoint: null,
      input: { taskId: 'task-1' },
      result: null,
      error: null,
      attempt: 0,
      leaseOwner: 'tok-1',
      leaseExpiresAt: this.iso(this.t + 30_000),
      approvalKey: null,
      decision: null,
      createdAt: this.iso(),
      updatedAt: this.iso(),
      startedAt: this.iso(),
      finishedAt: null,
      ...over,
    }
    this.rows.set(run.id, run)
    return { ...run }
  }

  async insert(row: NewRun): Promise<RunRow> {
    return this.running({ id: row.id, kind: row.kind, state: 'queued', leaseOwner: null, leaseExpiresAt: null, startedAt: null })
  }

  async get(id: string): Promise<RunRow | null> {
    const run = this.rows.get(id)
    return run ? { ...run } : null
  }

  async claim({ id, token, leaseMs }: { id: string; token: string; leaseMs: number }): Promise<ClaimResult> {
    const run = this.rows.get(id)
    if (!run) return { ok: false, reason: 'missing' }
    const live = run.leaseExpiresAt !== null && new Date(run.leaseExpiresAt).getTime() > this.t
    if ((run.state !== 'queued' && run.state !== 'running') || live) {
      if (run.state === 'queued' || run.state === 'running') return { ok: false, reason: 'taken', state: run.state, until: run.leaseExpiresAt }
      return { ok: false, reason: 'not-runnable', state: run.state }
    }
    const reclaimed = run.state === 'running'
    if (reclaimed) run.attempt += 1
    run.state = 'running'
    run.leaseOwner = token
    run.leaseExpiresAt = this.iso(this.t + leaseMs)
    run.startedAt ??= this.iso()
    return { ok: true, run: { ...run }, reclaimed }
  }

  async heartbeat({ id, token, leaseMs }: { id: string; token: string; leaseMs: number }): Promise<WriteResult> {
    return this.cas(id, token, (run) => void (run.leaseExpiresAt = this.iso(this.t + leaseMs)))
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

  async phase({ id, token, phase }: { id: string; token: string; phase: string }): Promise<WriteResult> {
    return this.cas(id, token, (run) => void (run.phase = phase))
  }

  async complete({ id, token, result }: { id: string; token: string; result: unknown }): Promise<WriteResult> {
    return this.cas(id, token, (run) => {
      run.state = 'done'
      run.result = result
      run.decision = null
      run.approvalKey = null
      run.leaseOwner = null
      run.leaseExpiresAt = null
      run.finishedAt = this.iso()
    })
  }

  async fail({ id, token, error }: { id: string; token: string; error: string }): Promise<WriteResult> {
    return this.cas(id, token, (run) => {
      run.state = 'error'
      run.error = error
      run.leaseOwner = null
      run.leaseExpiresAt = null
      run.finishedAt = this.iso()
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

  async defer({ id, token, until, reason }: { id: string; token: string; until: number; reason: string }): Promise<WriteResult> {
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

  async answer({ id, answer }: { id: string; answer: DecisionAnswer }) {
    const run = this.rows.get(id)
    if (!run) return { ok: false as const, reason: 'missing' as const }
    if (run.state !== 'awaiting') return { ok: false as const, reason: 'not-awaiting' as const, state: run.state }
    if (run.decision?.request.key !== answer.key) return { ok: false as const, reason: 'stale-key' as const, state: run.state }
    run.decision = { request: run.decision.request, answer }
    run.state = 'queued'
    run.leaseOwner = null
    run.leaseExpiresAt = null
    run.updatedAt = this.iso()
    return { ok: true as const, run: { ...run } }
  }

  async cancel({ id, reason }: { id: string; reason?: string }) {
    const run = this.rows.get(id)
    if (!run) return { ok: false as const, reason: 'missing' as const }
    if (run.state !== 'queued' && run.state !== 'running' && run.state !== 'awaiting') return { ok: false as const, reason: 'terminal' as const, state: run.state }
    const was: RunState = run.state
    run.state = 'cancelled'
    run.error = reason ?? null
    run.leaseOwner = null
    run.leaseExpiresAt = null
    return { ok: true as const, state: was }
  }

  async due(): Promise<RunRow[]> {
    return []
  }

  async activeFor(): Promise<RunRow[]> {
    return []
  }
}

class MemoryLease implements RunLease {
  held = new Map<string, string>()
  private n = 0
  async acquire(runId: string): Promise<LeaseClaim> {
    if (this.held.has(runId)) return { ok: false, reason: 'busy' }
    const token = `lease-${++this.n}`
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

// ── The run under test ───────────────────────────────────────────────────────

const ASK: DecisionRequest = {
  key: 'assignee',
  question: 'Two people are assigned — who should take this ticket?',
  detail: 'Both are editors on the board and neither has started.',
  options: [
    { id: 'ana', label: 'Ana', detail: 'Has the most context' },
    { id: 'ben', label: 'Ben' },
  ],
  href: '/boards/board-1/task-1',
}

/** Entered with a decision the person gave, so an assertion can prove the answer
 *  reached the step rather than merely reaching the row. */
const stepsSaw: Array<DecisionAnswer | null> = []

const HANDOVER = registerRun(
  defineRun<{ taskId: string }, { asked: boolean }>({
    kind: 'test-decision-run',
    label: 'Ticket handover',
    maxStepMs: 30_000,
    // THE AUTHORITY BOUNDARY, from the run's side: it names an authority and
    // knows nothing else about access. This one pauses to the ticket's BOARD.
    audience: (run) => ({ by: 'board', boardId: run.subjectId ?? 'unknown' }),
    step: async (ctx) => {
      stepsSaw.push(ctx.decision)
      if (ctx.decision) return { kind: 'done', result: { picked: ctx.decision.optionId } }
      return { kind: 'decide', question: ASK }
    },
  }),
)

interface World {
  store: MemoryStore
  lease: MemoryLease
  events: RunEvent[]
  announced: string[]
  askedFor: unknown[]
  deps: Partial<RunDeps> & { announce?: (key: string) => Promise<number> }
}

/** `content` is who the board resolves to; `reached` is what the announcer says
 *  it managed to tell, so a test can put the announcement on the floor. */
function world(content: string[] = ['u-editor'], reached = 1): World {
  const store = new MemoryStore()
  const lease = new MemoryLease()
  const events: RunEvent[] = []
  const announced: string[] = []
  const askedFor: unknown[] = []
  return {
    store,
    lease,
    events,
    announced,
    askedFor,
    deps: {
      store,
      lease,
      publish: (event: RunEvent) => void events.push(event),
      audienceFor: async (authority) => {
        askedFor.push(authority)
        return { content, fact: content.length ? [] : ['u-admin'] }
      },
      definitionFor: (kind: string) => (kind === HANDOVER.kind ? (HANDOVER as never) : null),
      now: () => NOW,
      announce: async (key: string) => {
        announced.push(key)
        return reached
      },
    },
  }
}

beforeEach(() => {
  stepsSaw.length = 0
  notifications.length = 0
  statements.length = 0
})

describe('runs: pause', () => {
  it('parks the run and files the approval to the audience the definition declares', async () => {
    const w = world(['u-editor'])
    const run = w.store.running()

    const res = await pause({ runId: run.id, token: 'tok-1', question: ASK }, w.deps)

    expect(res.ok).toBe(true)
    if (!res.ok) return
    // The key is derived from the run and the question and nothing else, so a
    // re-ask after a reclaim produces the same one and dedupes.
    expect(res.approvalKey).toBe('run:test-decision-run:run-1:assignee')
    expect(res.approvalKey).toBe(runApprovalKey(run, ASK.key))
    expect(res.announced).toBe(1)
    expect(w.announced).toEqual([res.approvalKey])
    // Resolved from the DEFINITION's authority — the board the run's subject is
    // on, not the run's owner and not the admins.
    expect(w.askedFor).toEqual([{ by: 'board', boardId: 'board-1' }])
    expect(res.audience.content).toEqual(['u-editor'])

    const row = w.store.rows.get(run.id)!
    expect(row.state).toBe('awaiting')
    expect(row.approvalKey).toBe(res.approvalKey)
    expect(row.decision).toEqual({ request: ASK, answer: null })
    // The lease is given back: nobody is driving a run that is waiting for a
    // person, and a lease held across a human's lunch break would either expire
    // and look reclaimable or have to be renewed by a process with no work.
    expect(row.leaseOwner).toBeNull()

    // Persisted, THEN published — and the question rides along so a device can
    // raise it without a round trip.
    const parked = w.events.at(-1)!
    expect(parked.state).toBe('awaiting')
    expect(parked.question).toEqual(ASK)
  })

  it('refuses to park a run this driver no longer holds, and tells nobody', async () => {
    const w = world()
    const run = w.store.running({ leaseOwner: 'somebody-else' })

    const res = await pause({ runId: run.id, token: 'tok-1', question: ASK }, w.deps)

    expect(res).toEqual({ ok: false, reason: 'lease-lost', state: 'running' })
    expect(w.announced).toEqual([])
    expect(w.store.rows.get(run.id)!.state).toBe('running')
  })

  it('keeps the run parked when nobody could be told, and leaves the key unmarked for the sweep', async () => {
    // `reached` 0: the announcement went nowhere. The row is still the record —
    // a delivery that did not happen must not destroy the thing it was about.
    const w = world([], 0)
    const run = w.store.running()

    const res = await pause({ runId: run.id, token: 'tok-1', question: ASK }, w.deps)

    expect(res.ok).toBe(true)
    if (!res.ok) return
    expect(res.announced).toBe(0)
    expect(w.store.rows.get(run.id)!.state).toBe('awaiting')
  })
})

describe('runs: a paused run cannot self-resume', () => {
  it('is not drivable — only a decision moves it out of awaiting', async () => {
    const w = world()
    const run = w.store.running()
    await pause({ runId: run.id, token: 'tok-1', question: ASK }, w.deps)
    stepsSaw.length = 0

    const result = await drive(run.id, w.deps)

    expect(result.stop).toBe('not-runnable')
    expect(result.steps).toBe(0)
    // The step was never re-entered, so the question was not asked a second
    // time and no side effect of it repeated.
    expect(stepsSaw).toEqual([])
    expect(w.store.rows.get(run.id)!.state).toBe('awaiting')
    // It never even took the lease.
    expect(w.lease.held.size).toBe(0)
  })
})

describe('runs: decide', () => {
  async function parked(content: string[] = ['u-editor']): Promise<World> {
    const w = world(content)
    const run = w.store.running()
    await pause({ runId: run.id, token: 'tok-1', question: ASK }, w.deps)
    stepsSaw.length = 0
    w.events.length = 0
    return w
  }

  it('refuses somebody the run’s authority does not name', async () => {
    const w = await parked(['u-editor'])

    const res = await decide({ runId: 'run-1', optionId: 'ana', by: 'u-stranger', start: false }, w.deps)

    expect(res).toEqual({ ok: false, reason: 'forbidden', state: 'awaiting' })
    const row = w.store.rows.get('run-1')!
    expect(row.state).toBe('awaiting')
    expect(row.decision?.answer).toBeNull()
    // Nothing about the question was published to anybody on the way out.
    expect(w.events).toEqual([])
  })

  it('refuses an option the step never offered', async () => {
    const w = await parked()

    const res = await decide({ runId: 'run-1', optionId: 'delete-the-board', by: 'u-editor', start: false }, w.deps)

    // The decider is entitled to answer and still cannot hand the step an
    // instruction it wrote no branch for: the answer is DATA, drawn from the
    // options the step declared.
    expect(res).toEqual({ ok: false, reason: 'unknown-option', state: 'awaiting' })
    expect(w.store.rows.get('run-1')!.decision?.answer).toBeNull()
  })

  it('writes the answer onto the run, re-queues it, and hands it to the next step', async () => {
    const w = await parked()

    const res = await decide({ runId: 'run-1', optionId: 'ana', note: '  Ana has the context  ', by: 'u-editor', start: false }, w.deps)

    expect(res.ok).toBe(true)
    if (!res.ok) return
    const row = w.store.rows.get('run-1')!
    expect(row.state).toBe('queued')
    expect(row.decision?.answer).toEqual({
      key: 'assignee',
      optionId: 'ana',
      note: 'Ana has the context',
      answeredBy: 'u-editor',
      answeredAt: new Date(NOW).toISOString(),
    })
    // Back in the queue with the lease clear: any instance may pick it up.
    expect(row.leaseOwner).toBeNull()

    // And the answer reaches the STEP, which is the only place it means
    // anything — then is cleared by the write that finishes the run, so a
    // reclaim cannot hand it to a step a second time.
    const driven = await drive('run-1', w.deps)
    expect(driven.stop).toBe('done')
    expect(stepsSaw.map((d) => d?.optionId)).toEqual(['ana'])
    expect(w.store.rows.get('run-1')!.result).toEqual({ picked: 'ana' })
    expect(w.store.rows.get('run-1')!.decision).toBeNull()
  })

  it('refuses a second answer to a question that has already been answered', async () => {
    const w = await parked()
    await decide({ runId: 'run-1', optionId: 'ana', by: 'u-editor', start: false }, w.deps)

    const second = await decide({ runId: 'run-1', optionId: 'ben', by: 'u-editor', start: false }, w.deps)

    expect(second).toEqual({ ok: false, reason: 'not-awaiting', state: 'queued' })
    expect(w.store.rows.get('run-1')!.decision?.answer?.optionId).toBe('ana')
  })
})

// ── THE PAUSE PROPERTY ───────────────────────────────────────────────────────
//
// The second of the two things the runs runtime exists to make true, asserted
// as ONE arc rather than in pieces, because the pieces passing individually is
// exactly how a system ends up with a run that parks correctly, announces to
// the wrong people, and resumes without the answer.
//
// Everything here is the real code: the real driver, the real `pause` it now
// delegates its park to, the real `runDecisionApproval` translation, the real
// `sweepUnannounced` and the real `mayDecide`. Only the store, the lease and
// the notification sink are fakes, and the store reimplements the compare-and-
// set predicates rather than accepting what it is told.
describe('PAUSE: a run parks on a person and comes back with their answer', () => {
  it('pauses into an approval, tells the audience its definition declared, refuses a stranger, and resumes with the answer in hand', async () => {
    const w = world(['u-editor'])
    // A fresh run nobody has claimed — so the DRIVER takes the lease and runs
    // the step, and the pause below is the one the step actually asked for.
    const run = w.store.running({ state: 'queued', leaseOwner: null, leaseExpiresAt: null, startedAt: null })

    // ── 1. It parks rather than guessing ────────────────────────────────────
    const first = await drive(run.id, w.deps)
    expect(first.stop).toBe('awaiting')
    expect(first.steps).toBe(1)
    const parked = w.store.rows.get(run.id)!
    expect(parked.state).toBe('awaiting')
    // Nothing is burning and nothing is held: the lease went back, so the row
    // is not going to look reclaimable while a person thinks about it.
    expect(parked.leaseOwner).toBeNull()
    expect(w.lease.held.size).toBe(0)
    // The question is ON THE ROW. Park on one instance, open the approval on
    // your phone: the question has to have survived the process that raised it.
    expect(parked.decision).toEqual({ request: ASK, answer: null })
    expect(parked.approvalKey).toBe(runApprovalKey(run, ASK.key))

    // ── 2. The right people are told, through the one announce path ─────────
    // Not the run's owner (u-owner) and not the admins: the definition's
    // authority is the ticket's BOARD, and this is what that resolved to.
    const approval = runDecisionApproval(parked)!
    const census = {
      approvals: [approval],
      failedKinds: [],
      audience: new Map([[approval.key, { content: ['u-editor'], fact: [] }]]),
    }
    expect(await sweepUnannounced(census, new Date(NOW), 60)).toEqual({ announced: 1, factOnly: 0, unreachable: 0, awaitingOwner: 0 })
    expect(notifications.map((n) => `${n.userId}:${n.kind}`)).toEqual(['u-editor:approval_pending'])
    expect(notifications[0]?.title).toContain(ASK.question)

    // ── 3. A stranger cannot answer it ──────────────────────────────────────
    expect(await decide({ runId: run.id, optionId: 'ana', by: 'u-stranger', start: false }, w.deps)).toEqual({
      ok: false,
      reason: 'forbidden',
      state: 'awaiting',
    })
    expect(w.store.rows.get(run.id)!.state).toBe('awaiting')
    expect(w.store.rows.get(run.id)!.decision?.answer).toBeNull()

    // ── 4. Somebody entitled does ───────────────────────────────────────────
    stepsSaw.length = 0
    const decided = await decide({ runId: run.id, optionId: 'ana', note: 'Ana has the context', by: 'u-editor', start: false }, w.deps)
    expect(decided.ok).toBe(true)
    const queued = w.store.rows.get(run.id)!
    expect(queued.state).toBe('queued')
    expect(queued.leaseOwner).toBeNull() // any instance may pick it up — not just the one that asked
    expect(queued.decision?.answer?.optionId).toBe('ana')
    expect(queued.decision?.answer?.answeredBy).toBe('u-editor')

    // ── 5. And the run resumes WITH the answer ──────────────────────────────
    const second = await drive(run.id, w.deps)
    expect(second.stop).toBe('done')
    // The answer reached the STEP, which is the only place it means anything.
    expect(stepsSaw.map((d) => d && `${d.optionId} by ${d.answeredBy}`)).toEqual(['ana by u-editor'])
    expect(w.store.rows.get(run.id)!.result).toEqual({ picked: 'ana' })
    // Cleared by the write that recorded the progress it produced, so a reclaim
    // cannot hand a step an answer it has already acted on.
    expect(w.store.rows.get(run.id)!.decision).toBeNull()
    expect(w.store.rows.get(run.id)!.approvalKey).toBeNull()
  })
})

describe('approvals: run_decision', () => {
  it('describes a parked run as an approval, with the authority its definition declared', async () => {
    const w = world()
    const run = w.store.running()
    await pause({ runId: run.id, token: 'tok-1', question: ASK }, w.deps)

    const approval = runDecisionApproval(w.store.rows.get(run.id)!)!

    expect(approval.kind).toBe('run_decision')
    // The key on the ROW, not a second derivation of it: the announce marks are
    // keyed on this string, and two spellings would announce the same pause
    // twice.
    expect(approval.key).toBe('run:test-decision-run:run-1:assignee')
    expect(approval.title).toBe('Ticket handover needs a decision: Two people are assigned — who should take this ticket?')
    expect(approval.detail).toContain('Options: Ana · Ben.')
    expect(approval.href).toBe('/boards/board-1/task-1')
    expect(approval.authority).toEqual({ by: 'board', boardId: 'board-1' })
    expect(approval.ownerUserIds).toEqual(['u-owner'])
  })

  it('is swept when it was never announced, so nobody was told exactly once', async () => {
    const w = world()
    const run = w.store.running()
    await pause({ runId: run.id, token: 'tok-1', question: ASK }, w.deps)
    const approval = runDecisionApproval(w.store.rows.get(run.id)!)!

    const census = {
      approvals: [approval],
      failedKinds: [],
      audience: new Map([[approval.key, { content: ['u-editor'], fact: [] }]]),
    }
    const result = await sweepUnannounced(census, new Date(NOW), 60)

    expect(result).toEqual({ announced: 1, factOnly: 0, unreachable: 0, awaitingOwner: 0 })
    expect(notifications).toEqual([
      {
        userId: 'u-editor',
        kind: 'approval_pending',
        title: approval.title,
        body: `${approval.detail}\n\nNothing happens until someone approves or rejects it.`,
        href: approval.href,
      },
    ])
    // Marked in the same pass, so the next tick does not say it again.
    expect(statements.some((s) => s.includes('approval_announce_state') || s.includes('app_settings'))).toBe(true)
  })
})
