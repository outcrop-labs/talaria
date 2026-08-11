import { describe, expect, it } from 'vitest'
import { bandFor, harnessSummary, slotsByHarness, tokensFor, valueOf, valueView, workloadFrom, type ValueDeps } from '@/server/fitness/value'
import type { FitnessIndex, FitnessIndexEntry, TokenBudget } from '@/server/fitness/surface'
import type { SlotBinding } from '@/server/fitness/score'
import type { ObservedHarness } from '@/server/fitness/observed'
import type { RegisteredHarness } from '@/server/harness/registry'

// ── Fixtures ─────────────────────────────────────────────────────────────────

/** `value.ts` reads exactly two fields off a registered harness — its id and
 *  whether it has any fixtures — so the fixture states those and casts. Building
 *  a real `RegisteredHarness` here (as surface.test.ts must, for `use`) would
 *  add a definition, a schema and a renderer to a test about arithmetic. */
const harness = (id: string, fixtures = 2): RegisteredHarness =>
  ({ id, label: id, evalNames: Array.from({ length: fixtures }, (_, i) => `case-${i}`) }) as unknown as RegisteredHarness

const observed = (harnessId: string, runs: number, model = 'a'): ObservedHarness =>
  ({ harness: harnessId, model, runs }) as unknown as ObservedHarness

const entry = (over: Partial<FitnessIndexEntry> = {}): FitnessIndexEntry => ({
  model: 'm',
  at: '2026-08-01T00:00:00.000Z',
  tiers: ['evals'],
  guarded: true,
  speed: null,
  cells: {},
  safety: null,
  probesWrote: 0,
  costUsd: null,
  calls: 0,
  partial: false,
  ...over,
})

const slotBinding = (kind: 'role' | 'agent', id: string, harnesses: string[], label = id): SlotBinding =>
  ({
    slot: { kind, id, label, hint: '', requires: [], live: true },
    harnesses: harnesses.map((h) => ({ id: h, via: 'chain' as const })),
  }) as unknown as SlotBinding

// ── The workload ─────────────────────────────────────────────────────────────

describe('workloadFrom', () => {
  it('divides observed runs by the window, summing across the models that served each harness', () => {
    const w = workloadFrom([observed('ticket', 300, 'a'), observed('ticket', 300, 'b'), observed('brief', 30)], [harness('ticket'), harness('brief')], 30)

    expect(w.basis).toBe('observed')
    expect(w.runs.ticket).toBe(20)
    expect(w.runs.brief).toBe(1)
    expect(w.perDay).toBe(21)
    expect(w.harnesses).toBe(2)
  })

  it('drops volume from a harness the registry no longer has', () => {
    // Rows outlive the code. Counting a deleted harness would put runs in the
    // denominator that no model can ever be scored on, which reads on the page
    // as every model getting worse.
    const w = workloadFrom([observed('ticket', 30), observed('deleted-last-year', 3000)], [harness('ticket')], 30)

    expect(Object.keys(w.runs)).toEqual(['ticket'])
    expect(w.perDay).toBe(1)
  })

  it('counts traffic on an unfixtured harness, and says how much', () => {
    // Real work nobody can score. It belongs in the denominator — a page that
    // dropped it would report 100% coverage of a day it only saw half of.
    const w = workloadFrom([observed('ticket', 30), observed('unscorable', 90)], [harness('ticket'), harness('unscorable', 0)], 30)

    expect(w.perDay).toBe(4)
    expect(w.unfixturedPerDay).toBe(3)
  })

  it('falls back to one run of every fixtured harness when production has nothing', () => {
    const w = workloadFrom([], [harness('ticket'), harness('brief'), harness('unscorable', 0)], 30)

    expect(w.basis).toBe('uniform')
    expect(w.perDay).toBe(2)
    expect(w.runs.unscorable).toBeUndefined()
    // The uniform basis invents no traffic for what it cannot score, so it has
    // no hole to report either.
    expect(w.unfixturedPerDay).toBe(0)
  })
})

// ── Tokens ───────────────────────────────────────────────────────────────────

describe('tokensFor', () => {
  const budget: TokenBudget = { ticket: { prompt: 1000, completion: 100, at: 'x' } }

  it("prefers this model's own measured tokens over the shared budget", () => {
    const e = entry({ harnesses: { ticket: { band: 'ready', cases: 4, prompt: 900, completion: 400 } } })

    expect(tokensFor('ticket', e, budget)).toEqual({ prompt: 900, completion: 400, basis: 'model' })
  })

  it('falls back to the shared budget when the model ran no cases on that harness', () => {
    // `cases: 0` with a band means the report judged the harness but the sweep
    // never called it. Pricing that at zero tokens would make a skipped harness
    // look free.
    const e = entry({ harnesses: { ticket: { band: 'untested', cases: 0, prompt: 0, completion: 0 } } })

    expect(tokensFor('ticket', e, budget)).toEqual({ prompt: 1000, completion: 100, basis: 'shared' })
  })

  it('reports nothing measured rather than zero', () => {
    expect(tokensFor('brief', entry(), budget)).toEqual({ prompt: 0, completion: 0, basis: 'none' })
  })

  it('refuses to read an all-zero entry as a measurement of zero', () => {
    // How this install lost its budget: a sweep against a model id the gateway
    // could not reach ran every case, failed every one before a token moved,
    // and wrote 0/0 for 26 harnesses. Read as real, it prints a confident
    // $0.00 for every model on the page.
    expect(tokensFor('ticket', entry(), { ticket: { prompt: 0, completion: 0, at: 'x' } }).basis).toBe('none')
    const zeroed = entry({ harnesses: { ticket: { band: 'unfit', cases: 4, prompt: 0, completion: 0 } } })
    expect(tokensFor('ticket', zeroed, {}).basis).toBe('none')
    // …and still falls through to a shared budget that DOES have numbers.
    expect(tokensFor('ticket', zeroed, budget)).toEqual({ prompt: 1000, completion: 100, basis: 'shared' })
  })
})

describe('harnessSummary', () => {
  const report = {
    model: 'm',
    guarded: true,
    unbound: [{ harness: 'subject-bound', band: 'ready' }],
    slots: [
      { slot: { kind: 'role', id: 'lenient' }, harnesses: [{ harness: 'ticket', band: 'ready' }] },
      { slot: { kind: 'role', id: 'strict' }, harnesses: [{ harness: 'ticket', band: 'workable' }] },
    ],
  } as unknown as Parameters<typeof harnessSummary>[0]

  it('averages tokens per case and keeps the worst band across slots', () => {
    const out = harnessSummary(report, [
      { id: 'ticket', cases: 4, promptTokens: 4000, completionTokens: 800 },
      { id: 'subject-bound', cases: 2, promptTokens: 1000, completionTokens: 500 },
    ])

    expect(out.ticket).toEqual({ band: 'workable', cases: 4, prompt: 1000, completion: 200 })
    // An unbound harness has no cell to derive from, which is the reason this
    // field exists at all.
    expect(out['subject-bound']).toEqual({ band: 'ready', cases: 2, prompt: 500, completion: 250 })
  })

  it('records a judged-but-unswept harness without dividing by zero', () => {
    const out = harnessSummary(report, [])

    expect(out.ticket).toEqual({ band: 'workable', cases: 0, prompt: 0, completion: 0 })
  })
})

// ── Bands ────────────────────────────────────────────────────────────────────

describe('bandFor', () => {
  const slotsOf = slotsByHarness([slotBinding('role', 'lenient', ['ticket']), slotBinding('role', 'strict', ['ticket'])])

  it("reads the entry's own per-harness band when it has one", () => {
    const e = entry({ harnesses: { ticket: { band: 'workable', cases: 4, prompt: 1, completion: 1 } }, cells: { 'role:lenient': { band: 'ready', reason: null } } })

    expect(bandFor('ticket', e, slotsOf)).toBe('workable')
  })

  it('falls back to the WORST cell across the slots a harness is bound to', () => {
    // Same numbers, two task floors. A permissive slot must not launder a
    // verdict the strict one refused.
    const e = entry({ cells: { 'role:lenient': { band: 'ready', reason: null }, 'role:strict': { band: 'workable', reason: null } } })

    expect(bandFor('ticket', e, slotsOf)).toBe('workable')
  })

  it('is untested for a model with no report, and for an unbound harness on an old entry', () => {
    expect(bandFor('ticket', undefined, slotsOf)).toBe('untested')
    expect(bandFor('subject-bound', entry({ cells: { 'role:lenient': { band: 'ready', reason: null } } }), slotsOf)).toBe('untested')
  })
})

// ── One model's row ──────────────────────────────────────────────────────────

describe('valueOf', () => {
  // 600 ticket runs and 30 brief runs over the 30-day window: 20/day and 1/day.
  const workload = workloadFrom([observed('ticket', 600), observed('brief', 30)], [harness('ticket'), harness('brief')], 30)
  const slotsOf = slotsByHarness([slotBinding('role', 'worker', ['ticket']), slotBinding('role', 'writer', ['brief'])])

  const tested = entry({
    harnesses: {
      ticket: { band: 'ready', cases: 4, prompt: 1000, completion: 200 },
      brief: { band: 'unfit', cases: 4, prompt: 500, completion: 1000 },
    },
  })

  it('prices the measured day and reports the shares it is priced over', () => {
    const v = valueOf({ model: 'm', entry: tested, price: { in: 1, out: 4 }, workload, budget: {}, slotsOf })

    // ticket: 20 runs × (1000 × $1 + 200 × $4) / 1e6 = $0.036
    // brief:   1 run  × (500  × $1 + 1000 × $4) / 1e6 = $0.0045
    expect(v.usdPerDay).toBeCloseTo(0.0405, 6)
    expect(v.readyShare).toBeCloseTo(20 / 21, 6)
    expect(v.shares.unfit).toBeCloseTo(1 / 21, 6)
    expect(Object.values(v.shares).reduce((a, b) => a + b, 0)).toBeCloseTo(1, 6)
    expect(v.costCoverage).toBe(1)
    expect(v.tokenBasis).toBe('model')
  })

  it('divides cost by the runs it is actually trusted with', () => {
    const v = valueOf({ model: 'm', entry: tested, price: { in: 1, out: 4 }, workload, budget: {}, slotsOf })

    expect(v.usdPerReadyRun).toBeCloseTo(0.0405 / 20, 8)
  })

  it('refuses a per-ready-run figure when the model is ready for nothing', () => {
    // The number would be a division by a zero the page cannot show, and the
    // honest reading is not "infinitely expensive" but "no answer".
    const useless = entry({ harnesses: { ticket: { band: 'unfit', cases: 4, prompt: 1000, completion: 200 } } })
    const v = valueOf({ model: 'm', entry: useless, price: { in: 1, out: 4 }, workload, budget: {}, slotsOf })

    expect(v.usdPerDay).toBeGreaterThan(0)
    expect(v.usdPerReadyRun).toBeNull()
  })

  it('reports coverage below 1 when part of the day has no measured tokens', () => {
    const partial = entry({ harnesses: { ticket: { band: 'ready', cases: 4, prompt: 1000, completion: 200 } } })
    const v = valueOf({ model: 'm', entry: partial, price: { in: 1, out: 4 }, workload, budget: {}, slotsOf })

    expect(v.costCoverage).toBeCloseTo(20 / 21, 6)
    expect(v.usdPerDay).toBeCloseTo(0.036, 6)
  })

  it('marks the basis shared as soon as one harness borrows the global budget', () => {
    const partial = entry({ harnesses: { ticket: { band: 'ready', cases: 4, prompt: 1000, completion: 200 } } })
    const v = valueOf({
      model: 'm',
      entry: partial,
      price: { in: 1, out: 4 },
      workload,
      budget: { brief: { prompt: 500, completion: 1000, at: 'x' } },
      slotsOf,
    })

    expect(v.costCoverage).toBe(1)
    expect(v.tokenBasis).toBe('shared')
  })

  it('leaves an unpriced model off the cost axis rather than at zero', () => {
    const v = valueOf({ model: 'm', entry: tested, price: null, workload, budget: {}, slotsOf })

    expect(v.usdPerDay).toBeNull()
    expect(v.usdPerReadyRun).toBeNull()
    // The performance half does not depend on a catalog being reachable.
    expect(v.readyShare).toBeCloseTo(20 / 21, 6)
  })

  it('reports a never-tested model as untested across the whole day', () => {
    const v = valueOf({ model: 'm', entry: undefined, price: { in: 1, out: 4 }, workload, budget: {}, slotsOf })

    expect(v.shares.untested).toBe(1)
    expect(v.readyShare).toBe(0)
    expect(v.at).toBeNull()
  })
})

// ── The whole view ───────────────────────────────────────────────────────────

describe('valueView', () => {
  const registry = [harness('ticket'), harness('brief')]
  const bindings = [slotBinding('role', 'worker', ['ticket'], 'Ticket worker'), slotBinding('role', 'writer', ['brief'], 'Writer')]

  const index: FitnessIndex = {
    cheap: entry({
      model: 'cheap',
      cells: { 'role:worker': { band: 'ready', reason: null }, 'role:writer': { band: 'unfit', reason: null } },
      harnesses: { ticket: { band: 'ready', cases: 4, prompt: 1000, completion: 200 }, brief: { band: 'unfit', cases: 4, prompt: 500, completion: 1000 } },
    }),
    dear: entry({
      model: 'dear',
      cells: { 'role:worker': { band: 'ready', reason: null }, 'role:writer': { band: 'ready', reason: null } },
      harnesses: { ticket: { band: 'ready', cases: 4, prompt: 1000, completion: 200 }, brief: { band: 'ready', cases: 4, prompt: 500, completion: 1000 } },
    }),
  }

  const deps = (over: Partial<ValueDeps> = {}): ValueDeps => ({
    observed: async () => [observed('ticket', 600), observed('brief', 30)],
    harnesses: async () => registry,
    bindings: async () => bindings,
    index: async () => index,
    budget: async () => ({}),
    price: async (model) => (model === 'cheap' ? { in: 0.1, out: 0.4 } : { in: 10, out: 40 }),
    record: async () => null,
    windowDays: 30,
    ...over,
  })

  it('ranks a slot by band first and price second, and recommends the cheapest Ready', async () => {
    const view = await valueView(deps())
    const worker = view.slots.find((s) => s.key === 'role:worker')!

    expect(worker.candidates.map((c) => c.model)).toEqual(['cheap', 'dear'])
    expect(worker.best).toBe('cheap')
    // The slot's own bill, not the whole workload's: ticket only.
    expect(worker.candidates[0]?.usdPerDay).toBeCloseTo((20 * (1000 * 0.1 + 200 * 0.4)) / 1e6, 8)
  })

  it('offers only the models that clear the floor, and says so by leaving the rest out', async () => {
    const view = await valueView(deps())
    const writer = view.slots.find((s) => s.key === 'role:writer')!

    expect(writer.candidates.map((c) => c.model)).toEqual(['dear'])
    expect(writer.best).toBe('dear')
    expect(writer.perDay).toBe(1)
  })

  it('does not let the cheaper model win the day when it cannot carry it', async () => {
    // The whole point of the second axis. `cheap` is 100× less per token and
    // covers 95% of the runs; `dear` covers all of them. Neither number alone
    // decides, and the page must carry both.
    const view = await valueView(deps())
    const cheap = view.models.find((m) => m.model === 'cheap')!
    const dear = view.models.find((m) => m.model === 'dear')!

    expect(cheap.usdPerDay!).toBeLessThan(dear.usdPerDay!)
    expect(cheap.readyShare).toBeLessThan(dear.readyShare)
    expect(dear.readyShare).toBe(1)
  })

  it('names the harnesses carrying volume that nothing has measured tokens for', async () => {
    const view = await valueView(
      deps({
        observed: async () => [observed('ticket', 30), observed('brief', 30), observed('unswept', 30)],
        harnesses: async () => [...registry, harness('unswept')],
      }),
    )

    expect(view.unmeasured).toEqual(['unswept'])
  })

  it('turns the cost axis off rather than drawing an unpriced fleet at zero', async () => {
    const view = await valueView(deps({ price: async () => null }))

    expect(view.priced).toBe(false)
    expect(view.models.every((m) => m.usdPerDay === null)).toBe(true)
    expect(view.slots.every((s) => s.candidates.every((c) => c.usdPerDay === null))).toBe(true)
  })

  it('renders on a fresh install, on the uniform basis, saying which basis it drew', async () => {
    const view = await valueView(deps({ observed: async () => [] }))

    expect(view.workload.basis).toBe('uniform')
    expect(view.workload.perDay).toBe(2)
    expect(view.models).toHaveLength(2)
  })

  it('survives a telemetry query that throws', async () => {
    // Advisory data behind a page that must still render — the same posture
    // observed.ts takes for the matrix.
    const view = await valueView(
      deps({
        observed: async () => {
          throw new Error('pg down')
        },
      }),
    )

    expect(view.workload.basis).toBe('uniform')
  })

  it('backfills an entry archived before the index carried its per-harness half', async () => {
    // The measurement is real and already paid for — it just lives in the full
    // report. Stranding it behind "re-test this model" bills an admin twice.
    const old = entry({ model: 'old', cells: { 'role:worker': { band: 'ready', reason: null } } })
    const view = await valueView(
      deps({
        index: async () => ({ old }),
        record: async () =>
          ({
            report: { model: 'old', guarded: true, unbound: [], slots: [{ slot: { kind: 'role', id: 'worker' }, harnesses: [{ harness: 'ticket', band: 'ready' }] }] },
            harnesses: [{ id: 'ticket', cases: 4, promptTokens: 4000, completionTokens: 800 }],
          }) as unknown as Awaited<ReturnType<ValueDeps['record']>>,
      }),
    )

    // Backfilled tokens, and this model's OWN — not the shared budget's.
    const row = view.models[0]!
    expect(row.tokenBasis).toBe('model')
    expect(row.usdPerDay).toBeCloseTo((20 * (1000 * 10 + 200 * 40)) / 1e6, 8)
  })

  it('reads the archive once per entry that needs it, and never for one that does not', async () => {
    const asked: string[] = []
    await valueView(
      deps({
        record: async (m) => {
          asked.push(m)
          return null
        },
      }),
    )

    // Both fixtures already carry `harnesses`, so nothing is read back.
    expect(asked).toEqual([])
  })

  it('falls back to the cells when even the archived report is gone', async () => {
    const old = entry({ model: 'old', cells: { 'role:worker': { band: 'ready', reason: null } } })
    const view = await valueView(deps({ index: async () => ({ old }), record: async () => null }))

    expect(view.models[0]?.readyShare).toBeCloseTo(20 / 21, 6)
    expect(view.models[0]?.tokenBasis).toBe('none')
  })

  it('lists only models something has tested', async () => {
    // Four hundred gateway ids at "untested, 0% ready" is true and is noise.
    const view = await valueView(deps())

    expect(view.models.map((m) => m.model)).toEqual(['cheap', 'dear'])
  })
})

describe('a price with nothing to price', () => {
  const workload = workloadFrom([observed('ticket', 600)], [harness('ticket')], 30)
  const slotsOf = slotsByHarness([slotBinding('role', 'worker', ['ticket'])])

  it('reports no figure rather than $0 a day', async () => {
    // A run that failed every case has a perfectly good $/MTok and not one
    // measured token. "$0 a day" would put it at the cheap end of the chart.
    const failed = entry({ harnesses: { ticket: { band: 'unfit', cases: 4, prompt: 0, completion: 0 } } })
    const v = valueOf({ model: 'm', entry: failed, price: { in: 10, out: 40 }, workload, budget: {}, slotsOf })

    expect(v.usdPerDay).toBeNull()
    expect(v.costCoverage).toBe(0)
    expect(v.tokenBasis).toBe('none')
  })

  it('says the same thing per slot', async () => {
    const index: FitnessIndex = {
      failed: entry({
        model: 'failed',
        cells: { 'role:worker': { band: 'ready', reason: null } },
        harnesses: { ticket: { band: 'ready', cases: 4, prompt: 0, completion: 0 } },
      }),
    }
    const view = await valueView({
      observed: async () => [observed('ticket', 600)],
      harnesses: async () => [harness('ticket')],
      bindings: async () => [slotBinding('role', 'worker', ['ticket'])],
      index: async () => index,
      budget: async () => ({}),
      price: async () => ({ in: 10, out: 40 }),
      record: async () => null,
      windowDays: 30,
    })

    expect(view.slots[0]?.candidates[0]?.usdPerDay).toBeNull()
  })
})
