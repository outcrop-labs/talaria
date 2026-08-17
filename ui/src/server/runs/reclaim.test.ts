import { afterEach, describe, expect, it, vi } from 'vitest'
import { defineRun, type AnyRunDefinition, type RunDefinition, type RunRow, type RunState } from '@/server/runs/define'
import {
  RECLAIM_EVERY_MS,
  RECLAIM_JOB_SPEC,
  RECLAIM_LIMIT,
  RECLAIM_MAX_RUN_MS,
  describeSweep,
  runReclaimJob,
  sweepReclaimableRuns,
  type ReclaimDeps,
} from '@/server/runs/reclaim'
import type { DriveResult } from '@/server/runs/run'

// The sweeper is exercised against an in-memory due-query, an in-memory
// definition registry and a fake driver, so nothing here touches Postgres or
// Redis. Every edge is a field on `ReclaimDeps` — same pattern and same reason
// as runs/run.test.ts and harness/run.test.ts.
//
// WHAT THESE TESTS ARE ACTUALLY FOR. The sweeper is the piece that makes
// "survives a restart" true, and its two failure modes are opposites: it can
// fail to wake a run whose driver died (durability silently does not happen),
// or it can wake — or worse, FAIL — a run that is perfectly healthy. The second
// is the one server/research.ts:352 ships today, and the `awaiting` case is the
// sharpest version of it: a run parked on a person may sit for days, and a
// sweeper that read "old and not moving" as "broken" would auto-fail every
// paused run in the product.

// The scheduler is mocked so importing this module does not register a real job
// into the process-wide registry. `RECLAIM_JOB_SPEC` is asserted directly
// instead, which is the fact worth pinning anyway: the timings the job declares.
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))

// ── The fake world ───────────────────────────────────────────────────────────

const NOW = 1_700_000_000_000
const iso = (t: number): string => new Date(t).toISOString()

const asAny = <I, C>(def: RunDefinition<I, C>): AnyRunDefinition => def as unknown as AnyRunDefinition

const DEF = asAny(
  defineRun<null, null>({
    kind: 'test-kind',
    label: 'Test kind',
    step: async () => ({ kind: 'done' }),
    audience: () => ({ by: 'admin' }),
    maxStepMs: 30_000,
  }),
)

/** A run row as the due query would hand it back. Defaults describe the
 *  ordinary reclaim case: `running`, lease expired a minute ago, first entry. */
function row(over: Partial<RunRow> = {}): RunRow {
  return {
    id: over.id ?? 'run-1',
    kind: 'test-kind',
    ownerUserId: 'user-1',
    subjectType: null,
    subjectId: null,
    state: 'running' as RunState,
    phase: 'reading the sources',
    checkpoint: { page: 3 },
    input: null,
    result: null,
    error: null,
    attempt: 0,
    leaseOwner: 'dead-driver-token',
    leaseExpiresAt: iso(NOW - 60_000),
    approvalKey: null,
    decision: null,
    createdAt: iso(NOW - 600_000),
    updatedAt: iso(NOW - 120_000),
    startedAt: iso(NOW - 600_000),
    finishedAt: null,
    ...over,
  }
}

const stop = (s: DriveResult['stop'], over: Partial<DriveResult> = {}): DriveResult => ({
  runId: 'run-1',
  stop: s,
  steps: 0,
  state: null,
  ...over,
})

interface Harness {
  deps: Partial<ReclaimDeps>
  /** Run ids handed to the driver, in order. */
  driven: string[]
  /** The limit the sweep asked the store for. */
  askedLimit: number | null
}

function harness(rows: RunRow[], opts: { drive?: (id: string) => Promise<DriveResult>; defs?: Map<string, AnyRunDefinition> } = {}): Harness {
  const h: Harness = { deps: {}, driven: [], askedLimit: null }
  const defs = opts.defs ?? new Map<string, AnyRunDefinition>([['test-kind', DEF]])
  h.deps = {
    due: async ({ limit }) => {
      h.askedLimit = limit
      return rows.slice(0, limit)
    },
    definitionFor: (kind) => defs.get(kind) ?? null,
    drive: async (runId) => {
      h.driven.push(runId)
      return opts.drive ? await opts.drive(runId) : stop('done', { runId, state: 'done' })
    },
    now: () => NOW,
  }
  return h
}

/** Detached drives are kicked with `void`, so the assertion has to come after
 *  the microtask queue has drained. One turn is enough — nothing in the fake
 *  driver waits on a timer. */
const settle = (): Promise<void> => new Promise((resolve) => setImmediate(resolve))

afterEach(() => {
  vi.restoreAllMocks()
})

// ── The reclaim itself ───────────────────────────────────────────────────────

describe('sweepReclaimableRuns', () => {
  it('re-queues a running run whose lease has expired', async () => {
    const h = harness([row()])
    const r = await sweepReclaimableRuns({}, h.deps)
    await settle()

    expect(r.scanned).toBe(1)
    expect(r.driven).toBe(1)
    // The number that says a PROCESS died, as opposed to a queued run nobody
    // had picked up yet. They are different operational facts.
    expect(r.reclaimed).toBe(1)
    expect(r.givenUp).toBe(0)
    expect(h.driven).toEqual(['run-1'])
    // The lease had been expired for a minute; that is the queue-depth number.
    expect(r.stalestMs).toBe(60_000)
  })

  it('hands over a queued run with no lease, and does not call it a reclaim', async () => {
    // A run that was enqueued and never claimed — the process that inserted it
    // died before its detached drive got going, or never had one.
    const h = harness([row({ state: 'queued', leaseOwner: null, leaseExpiresAt: null })])
    const r = await sweepReclaimableRuns({}, h.deps)
    await settle()

    expect(r.driven).toBe(1)
    expect(r.reclaimed).toBe(0)
    expect(h.driven).toEqual(['run-1'])
  })

  it('leaves a run whose lease is still live alone', async () => {
    // The store's query already excludes these. The sweeper says it a second
    // time because it is the one thing in the system that can wake a run from
    // the outside, and two drivers in one run is how a side effect happens
    // twice.
    const h = harness([row({ leaseExpiresAt: iso(NOW + 30_000) })])
    const r = await sweepReclaimableRuns({}, h.deps)
    await settle()

    expect(r.live).toBe(1)
    expect(r.driven).toBe(0)
    expect(h.driven).toEqual([])
  })

  it('leaves a run for another instance when this one has no definition for the kind', async () => {
    // A row from a newer deploy, or a module not in this process's graph. NOT
    // an error on the row: failing it here would destroy work on the strength
    // of a local import graph.
    const h = harness([row({ kind: 'kind-from-the-future' })])
    const r = await sweepReclaimableRuns({}, h.deps)
    await settle()

    expect(r.unknownKinds).toBe(1)
    expect(r.driven).toBe(0)
    expect(h.driven).toEqual([])
  })
})

// ── The guard the whole file exists for ──────────────────────────────────────

describe('awaiting is never reclaimed', () => {
  it('never drives and never fails a run parked on a person, however long it has sat', async () => {
    // Everything about this row screams "stale" to a sweeper that measures
    // staleness in wall-clock time: parked a week ago, no lease, and it has
    // already spent every attempt it had. It is still perfectly healthy — it is
    // waiting for somebody to answer a question — and this is the exact row
    // server/research.ts's sweep would mark
    // `error: 'run went stale (app restarted mid-research?)'`.
    const h = harness([
      row({
        state: 'awaiting',
        attempt: 99,
        leaseOwner: null,
        leaseExpiresAt: null,
        approvalKey: 'run:test-kind:run-1:pick-a-branch',
        updatedAt: iso(NOW - 7 * 24 * 60 * 60_000),
      }),
    ])
    const r = await sweepReclaimableRuns({}, h.deps)
    await settle()

    expect(r.notDrivable).toBe(1)
    expect(r.driven).toBe(0)
    expect(r.givenUp).toBe(0)
    expect(r.reclaimed).toBe(0)
    expect(h.driven).toEqual([])
  })

  it('leaves terminal runs alone too', async () => {
    const h = harness([row({ id: 'a', state: 'done' }), row({ id: 'b', state: 'error' }), row({ id: 'c', state: 'cancelled' })])
    const r = await sweepReclaimableRuns({}, h.deps)
    await settle()

    expect(r.notDrivable).toBe(3)
    expect(h.driven).toEqual([])
  })
})

// ── Giving up ────────────────────────────────────────────────────────────────

describe('attempts spent', () => {
  it('awaits the hand-over and reports the give-up with what actually happened', async () => {
    // attempt 2, and the claim will make it 3 — the default maximum. The driver
    // files the error; the sweep watches so the pass can SAY so, and adds what
    // only it knew: where the run died and how long ago.
    const errors: string[] = []
    vi.spyOn(console, 'error').mockImplementation((...args: unknown[]) => {
      errors.push(args.map((a) => String(a)).join(' '))
    })

    const h = harness([row({ attempt: 2, phase: 'summarizing 40 sources' })], {
      drive: async (runId) =>
        stop('exhausted', {
          runId,
          state: 'error',
          error: 'run gave up after 3 attempt(s): each driver that took it stopped without finishing or checkpointing',
        }),
    })
    const r = await sweepReclaimableRuns({}, h.deps)

    expect(r.givenUp).toBe(1)
    expect(r.driven).toBe(0)
    expect(h.driven).toEqual(['run-1'])

    const line = errors.find((e) => e.includes('gave up on run-1'))
    expect(line).toBeTruthy()
    // The count, the driver's own diagnosis, and the phase it died at. Not
    // "run went stale".
    expect(line).toContain('after 3 attempt(s)')
    expect(line).toContain('stopped without finishing or checkpointing')
    expect(line).toContain('summarizing 40 sources')
    expect(line).not.toContain('went stale')
  })

  it('still resumes a run one attempt short of the line', async () => {
    // The boundary, pinned from the other side: attempt 1 becomes 2 on the
    // claim, which is under the default of 3, so this run gets another go.
    const h = harness([row({ attempt: 1 })])
    const r = await sweepReclaimableRuns({}, h.deps)
    await settle()

    expect(r.driven).toBe(1)
    expect(r.givenUp).toBe(0)
  })

  it('counts a queued run without adding the reclaim increment', async () => {
    // `store.claim` bumps `attempt` only when the previous state was `running`.
    // A queued run at attempt 2 of 3 therefore has one entry left, and must not
    // be treated as spent.
    const h = harness([row({ state: 'queued', attempt: 2, leaseOwner: null, leaseExpiresAt: null })])
    const r = await sweepReclaimableRuns({}, h.deps)
    await settle()

    expect(r.driven).toBe(1)
    expect(r.givenUp).toBe(0)
  })

  it("honors a definition's own maxAttempts", async () => {
    const once = asAny(
      defineRun<null, null>({
        kind: 'no-second-chances',
        label: 'Runs once',
        step: async () => ({ kind: 'done' }),
        audience: () => ({ by: 'admin' }),
        maxStepMs: 1_000,
        maxAttempts: 1,
      }),
    )
    const h = harness([row({ kind: 'no-second-chances', attempt: 0 })], {
      defs: new Map([['no-second-chances', once]]),
      drive: async (runId) => stop('exhausted', { runId, state: 'error', error: 'run gave up after 1 attempt(s)' }),
    })
    const r = await sweepReclaimableRuns({}, h.deps)

    expect(r.givenUp).toBe(1)
    expect(r.driven).toBe(0)
  })

  it('counts a hand-over the driver did not treat as exhausted as an ordinary hand-over', async () => {
    // The give-up is the DRIVER's decision; the sweep only predicts it to
    // decide whether to await. A prediction that turns out wrong — another
    // instance claimed the run first — must not be reported as a give-up.
    const h = harness([row({ attempt: 2 })], { drive: async (runId) => stop('busy', { runId, state: 'running' }) })
    const r = await sweepReclaimableRuns({}, h.deps)

    expect(r.givenUp).toBe(0)
    expect(r.driven).toBe(1)
    expect(r.reclaimed).toBe(1)
  })
})

// ── The bound ────────────────────────────────────────────────────────────────

describe('the pass is bounded', () => {
  it('asks the store for at most RECLAIM_LIMIT runs by default', async () => {
    const h = harness([])
    await sweepReclaimableRuns({}, h.deps)
    expect(h.askedLimit).toBe(RECLAIM_LIMIT)
  })

  it('never starts more drives than the bound, however many runs are due', async () => {
    const many = Array.from({ length: 100 }, (_, i) => row({ id: `run-${i}` }))
    const h = harness(many)
    const r = await sweepReclaimableRuns({ limit: 3 }, h.deps)
    await settle()

    expect(h.askedLimit).toBe(3)
    expect(r.scanned).toBe(3)
    expect(h.driven).toEqual(['run-0', 'run-1', 'run-2'])
  })

  it('clamps a nonsensical limit rather than asking for zero rows', async () => {
    const h = harness([row()])
    await sweepReclaimableRuns({ limit: 0 }, h.deps)
    expect(h.askedLimit).toBe(1)
  })
})

// ── Being visible when it stops working ──────────────────────────────────────

describe('the job', () => {
  it('declares timings the scheduler can call hung', () => {
    expect(RECLAIM_JOB_SPEC.name).toBe('run-reclaim')
    expect(RECLAIM_JOB_SPEC.everyMs).toBe(RECLAIM_EVERY_MS)
    expect(RECLAIM_JOB_SPEC.maxRunMs).toBe(RECLAIM_MAX_RUN_MS)
    // `unhealthyJobs()` calls a run past `maxRunMs` HUNG rather than slow, and
    // it only ever gets to ask if the number is declared. A pass with no bound
    // is the failure the scheduler header names: failures 0, runs 0, running
    // true forever, and every tick behind it turned away.
    expect(RECLAIM_JOB_SPEC.maxRunMs).toBeGreaterThan(0)
    // Not perInstance: the input is a shared table, so the fleet does one sweep
    // per interval rather than one per instance. See JobSpec.perInstance.
    expect(RECLAIM_JOB_SPEC.perInstance).toBeUndefined()
    // A settle window before an instance starts re-entering steps that can bill
    // model calls — a crash-looping box must not reach this job.
    expect(RECLAIM_JOB_SPEC.firstRunDelayMs).toBeGreaterThan(0)
  })

  it('reports nothing to do as nothing to do', async () => {
    const h = harness([])
    await expect(runReclaimJob(h.deps)).resolves.toBeNull()
  })

  it('returns a sentence naming what the pass did', async () => {
    const h = harness([row(), row({ id: 'run-2', state: 'awaiting', leaseExpiresAt: null })])
    const line = await runReclaimJob(h.deps)
    await settle()

    expect(line).toContain('2 run(s) due')
    expect(line).toContain('1 reclaimed from a driver that died')
    expect(line).toContain('1 not drivable')
  })

  it('THROWS when a hand-over throws, so the failure reaches /observability', async () => {
    // `drive` is total for everything a run can do to itself. A throw out of it
    // means the store or the lease is broken under the whole runtime, and the
    // scheduler's error state is the only place that fact reaches a person.
    vi.spyOn(console, 'error').mockImplementation(() => {})
    const h = harness([row({ attempt: 2 })], {
      drive: async () => {
        throw new Error('connection pool exhausted')
      },
    })
    await expect(runReclaimJob(h.deps)).rejects.toThrow(/hand-over\(s\) threw/)
  })

  it('lets a failing due query throw rather than reporting a quiet empty pass', async () => {
    // A sweeper that cannot see the queue reports zero runs due, which reads
    // exactly like a healthy idle fleet. That silence is the disease.
    const deps: Partial<ReclaimDeps> = {
      due: async () => {
        throw new Error('relation "runs" does not exist')
      },
    }
    await expect(runReclaimJob(deps)).rejects.toThrow(/relation "runs" does not exist/)
  })

  it('does not let a detached drive that throws take the process down', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {})
    const h = harness([row()], {
      drive: async () => {
        throw new Error('redis is on fire')
      },
    })
    // attempt 0, so the hand-over is detached: the pass resolves cleanly and
    // the rejection is caught at the kick site rather than becoming an
    // unhandled rejection.
    await expect(sweepReclaimableRuns({}, h.deps)).resolves.toMatchObject({ driven: 1, failed: 0 })
    await settle()
  })
})

describe('describeSweep', () => {
  it('says only what happened', () => {
    const line = describeSweep({
      scanned: 4,
      driven: 2,
      reclaimed: 1,
      givenUp: 1,
      unknownKinds: 1,
      live: 0,
      notDrivable: 0,
      failed: 0,
      stalestMs: 125_000,
    })
    expect(line).toBe(
      '4 run(s) due — 2 handed to a driver, 1 reclaimed from a driver that died, 1 given up on (attempts spent), ' +
        '1 of a kind this instance cannot drive, stalest lease expired 2 minutes ago',
    )
  })
})
