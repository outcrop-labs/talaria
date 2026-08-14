import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import {
  drilldown,
  estimateRun,
  evictArchive,
  forgetModel,
  indexEntryOf,
  INDEX_KEY,
  keysFor,
  MAX_CONCURRENT_RUNS,
  canonicalIndex,
  fitnessRuns,
  clearFitnessResults,
  liveCases,
  liveLog,
  LIVE_LOG_CAP,
  speedOf,
  runningModels,
  stopRequestedFor,
  storedIdFor,
  mergeFact,
  modelRows,
  nextBudget,
  priceOf,
  POOLED_DISAGREEMENT,
  recordKey,
  startFitnessRun,
  stopFitnessRun,
  usdOf,
  type FitnessIndex,
  type FitnessIndexEntry,
  type FitnessRunStatus,
  type TokenBudget,
  type SurfaceDeps,
} from '@/server/fitness/surface'
import type { CapabilityFact } from '@/server/harness/capability'
import type { EvalCaseScore, EvalSweep, HarnessScore } from '@/server/fitness/evals'
import type { ProbeEstimate, ProbeReport } from '@/server/fitness/probes'
import type { AdversarialEstimate } from '@/server/fitness/adversarial'
import type { FitnessReport } from '@/server/fitness/score'
import type { LlmEndpoint } from '@/server/agent-defs'
import { defineHarness, type HarnessDefinition } from '@/server/harness/define'
import type { HarnessSource, RegisteredHarness } from '@/server/harness/registry'

// Everything under test here used to live in `routes/api/admin.model-fitness.ts`,
// where `vitest.config.ts` cannot reach it — see the comment at the top of that
// config for why the exclusion stays and the code moves instead. These are the
// four decisions that were unreachable: what a capability tag says across a
// pooled endpoint set, what a run is going to cost, what the drill-down keeps,
// and what the archive throws away.

// ── Fixtures ─────────────────────────────────────────────────────────────────

const fact = (value: boolean, over: Partial<CapabilityFact> = {}): CapabilityFact => ({
  value,
  source: 'probe',
  at: '2026-08-01T00:00:00.000Z',
  ...over,
})

/** `registry.ts` keeps `register` private, so the shape is rebuilt — including
 *  `use`, which is the only way `tier2Shape` can ask a definition whether it
 *  can repair with its I and O still paired. */
function reg<I, O>(def: HarnessDefinition<I, O>, source: HarnessSource = 'builtin'): RegisteredHarness {
  return {
    id: def.id,
    label: def.label,
    job: def.job,
    source,
    requires: def.requires,
    floor: def.floor,
    model: def.model,
    outputKind: def.output.kind,
    tools: def.tools ?? 'none',
    bandOf: Object.fromEntries((def.evals ?? []).map((e) => [e.name, e.band ?? ('standard' as const)])),
    widen: def.widen ?? null,
    guard: def.guard ?? null,
    temperature: def.temperature ?? null,
    evalNames: (def.evals ?? []).map((e) => e.name),
    use: (fn) => fn(def),
  }
}

const FLOOR = { capabilities: [], refuseBelow: false, note: 'Runs on anything.' }

/** A JSON harness — repairable, so it contributes a repair turn to the ceiling. */
const jsonHarness = (id: string, fixtures: number): RegisteredHarness =>
  reg(
    defineHarness<{ n: number }, { out: string }>({
      id,
      label: id,
      job: 'test',
      requires: [],
      floor: FLOOR,
      model: { chain: [] },
      render: (input) => [{ role: 'user', content: `n=${input.n}` }],
      output: { kind: 'json', schema: z.object({ out: z.string() }) },
      onFailure: 'null',
      evals: Array.from({ length: fixtures }, (_, i) => ({ name: `case-${i}`, input: { n: i }, check: () => null })),
    }),
  )

/** A text harness — `run.ts` sets `maxRepairs` to 0 for these, so budgeting a
 *  repair turn for one would inflate every estimate on a mostly-text registry.
 *  This fixture is what proves the estimate does not. */
const textHarness = (id: string, fixtures: number): RegisteredHarness =>
  reg(
    defineHarness<{ n: number }, string>({
      id,
      label: id,
      job: 'test',
      requires: [],
      floor: FLOOR,
      model: { chain: [] },
      render: (input) => [{ role: 'user', content: `n=${input.n}` }],
      output: { kind: 'text' },
      onFailure: 'null',
      evals: Array.from({ length: fixtures }, (_, i) => ({ name: `case-${i}`, input: { n: i }, check: () => null })),
    }),
  )

const ep = (name: string, over: Partial<LlmEndpoint> = {}): LlmEndpoint => ({
  id: name,
  name,
  provider: 'openai-compatible',
  baseUrl: null,
  class: 'local',
  apiKeyEnv: null,
  hasKey: false,
  contextLength: 32_000,
  priceInPerMtok: null,
  priceOutPerMtok: null,
  models: [],
  modelPrices: {},
  autoPrices: {},
  requestDefaults: {},
  ...over,
})

const probeEstimate = (over: Partial<ProbeEstimate> = {}): ProbeEstimate => ({
  model: 'm',
  rows: [],
  calls: 0,
  known: 0,
  promptTokens: 0,
  completionTokens: 0,
  usd: null,
  ...over,
})

const adversarialEstimate = (over: Partial<AdversarialEstimate> = {}): AdversarialEstimate => ({
  calls: 0,
  adversaryCalls: 0,
  promptTokens: 0,
  completionTokens: 0,
  costUsd: null,
  worstCase: false,
  ...over,
})

/** Only the edges `estimateRun` actually reads. Everything else stays real and
 *  would throw if it were touched, which is the point. */
function estimateDeps(over: Partial<SurfaceDeps>): Partial<SurfaceDeps> {
  return {
    routing: async (model) => ({ endpoints: [], upstreamModel: model }),
    harnesses: async () => [],
    readSetting: async <T,>(_key: string, fallback: T): Promise<T> => fallback,
    estimateProbes: async () => probeEstimate(),
    estimateAdversarial: async () => adversarialEstimate(),
    ...over,
  }
}

const evalCase = (over: Partial<EvalCaseScore> = {}): EvalCaseScore => ({
  harness: 'h',
  case: 'c',
  band: 'standard',
  skipped: null,
  contractHeld: true,
  firstPass: true,
  repairs: 0,
  answered: true,
  task: 'pass',
  taskError: null,
  gap: null,
  findings: 0,
  latencyMs: 10,
  promptTokens: 10,
  completionTokens: 5,
  costUsd: null,
  estimated: false,
  timedOut: false,
  optimistic: false,
  error: null,
  prompt: null,
  raw: null,
  turns: null,
  calls: null,
  upstream: null,
  startedAt: '2026-08-01T00:00:00.000Z',
  wallMs: 0,
  ...over,
})

const entry = (model: string, at: string): FitnessIndexEntry => ({
  model,
  at,
  tiers: ['evals'],
  guarded: true,
  cells: {},
  safety: null,
  probesWrote: 0,
  speed: null,
  costUsd: null,
  calls: 0,
  partial: false,
})

// ── mergeFact ────────────────────────────────────────────────────────────────
//
// THE MOST CONSEQUENTIAL FUNCTION ON THE PAGE. It decides whether a capability
// tag shows at all for a model id, which is to say it decides what an admin
// believes about a model before they assign it a role. Both directions of the
// unknown rule are asserted here, because "unknown is not false" is what keeps
// a fresh self-host from rendering as a wall of red — and the disagreement rule
// is what keeps a pooled id from being credited with its best member's answer.

describe('mergeFact', () => {
  it('reports a lone measured fact verbatim, metadata and all', () => {
    const at = '2026-07-04T12:00:00.000Z'
    expect(mergeFact([fact(true, { detail: 'held json mode 3/3', score: 1, at })])).toEqual({
      state: 'yes',
      source: 'probe',
      detail: 'held json mode 3/3',
      score: 1,
      at, via: null })
  })

  it('reports a measured false as no — a recorded failure is a fact, not a gap', () => {
    expect(mergeFact([fact(false, { detail: 'returned prose', source: 'learned' })])).toMatchObject({
      state: 'no',
      source: 'learned',
      detail: 'returned prose',
    })
  })

  it('omits absent optional metadata as null rather than undefined', () => {
    // The row is serialized to the client; `score: undefined` would vanish from
    // the JSON and a panel reading `score === null` would never fire.
    expect(mergeFact([fact(true)])).toEqual({
      state: 'yes',
      source: 'probe',
      detail: null,
      score: null,
      at: '2026-08-01T00:00:00.000Z', via: null })
  })

  it('is unknown for a model nothing has measured', () => {
    expect(mergeFact([])).toEqual({ state: 'unknown', source: null, detail: null, score: null, at: null, via: null })
    expect(mergeFact([undefined])).toEqual({ state: 'unknown', source: null, detail: null, score: null, at: null, via: null })
  })

  it('carries an agreeing pool through — yes when every member says yes', () => {
    const merged = mergeFact([fact(true, { detail: 'first' }), fact(true, { detail: 'second' })])
    expect(merged.state).toBe('yes')
    // The first member's metadata, deliberately: the alternative is inventing a
    // consensus sentence no endpoint actually wrote.
    expect(merged.detail).toBe('first')
  })

  it('carries an agreeing pool through — no when every member says no', () => {
    expect(mergeFact([fact(false), fact(false), fact(false)]).state).toBe('no')
  })

  it('is unknown when the pool disagrees, in either direction', () => {
    const trueFirst = mergeFact([fact(true), fact(false)])
    const falseFirst = mergeFact([fact(false), fact(true)])
    // Order must not decide it. A vote — or "whichever member answered first" —
    // is exactly the false `true` that `runProbes` refuses to write, because a
    // bare id round-robins and the next call may land on the other member.
    expect(trueFirst).toEqual(falseFirst)
    expect(trueFirst.state).toBe('unknown')
    expect(trueFirst.detail).toBe(POOLED_DISAGREEMENT)
    expect(trueFirst.source).toBeNull()
    expect(trueFirst.at).toBeNull()
  })

  it('is unknown when one member of the pool has never been measured — not yes', () => {
    // A true from one endpoint says nothing about the endpoint nobody probed.
    const merged = mergeFact([fact(true), undefined])
    expect(merged.state).toBe('unknown')
    // No disagreement sentence: nothing disagreed, something is simply missing,
    // and telling an admin to "test the qualified id" over a gap they can close
    // by running the probes would be the wrong instruction.
    expect(merged.detail).toBeNull()
  })

  it('is unknown when one member of the pool has never been measured — not no', () => {
    // THE DIRECTION THAT MATTERS MOST. Collapsing a gap into `false` is how
    // every unprobed model on a fresh install turns red, and score.ts reads a
    // recorded `false` as UNFIT while an absent fact is merely untested.
    expect(mergeFact([fact(false), undefined]).state).toBe('unknown')
    expect(mergeFact([undefined, fact(false)]).state).toBe('unknown')
  })

  it('is unknown for a three-member pool with one gap even when the other two agree', () => {
    expect(mergeFact([fact(true), fact(true), undefined]).state).toBe('unknown')
  })
})

describe('modelRows', () => {
  const facts: Record<string, Partial<Record<'json' | 'tools', CapabilityFact>>> = {
    'spark:qwen3-14b': { json: fact(true), tools: fact(true) },
    'local:qwen3-14b': { json: fact(false) },
  }

  const deps: Partial<SurfaceDeps> = {
    models: async () => [
      { id: 'spark/qwen3-14b', endpoints: ['spark'], qualified: true },
      { id: 'local/qwen3-14b', endpoints: ['local'], qualified: true },
      { id: 'qwen3-14b', endpoints: ['spark', 'local'], qualified: false },
    ],
    capabilities: async (key) => facts[key] ?? {},
  }

  it('flags a bare id served by two endpoints as pooled and merges its facts', async () => {
    const rows = await modelRows(deps)
    const pooled = rows.find((r) => r.id === 'qwen3-14b')
    expect(pooled?.pooled).toBe(true)
    // json: measured on both and they disagree → unknown, with the sentence.
    expect(pooled?.capabilities.find((c) => c.cap === 'json')).toMatchObject({ state: 'unknown', detail: POOLED_DISAGREEMENT })
    // tools: measured on one only → unknown, and NOT the `yes` that one member
    // would have given on its own.
    expect(pooled?.capabilities.find((c) => c.cap === 'tools')).toMatchObject({ state: 'unknown', detail: null })
  })

  it('leaves the endpoint-qualified siblings answering for themselves', async () => {
    const rows = await modelRows(deps)
    expect(rows.find((r) => r.id === 'spark/qwen3-14b')?.capabilities.find((c) => c.cap === 'json')?.state).toBe('yes')
    expect(rows.find((r) => r.id === 'local/qwen3-14b')?.capabilities.find((c) => c.cap === 'json')?.state).toBe('no')
    expect(rows.find((r) => r.id === 'spark/qwen3-14b')?.pooled).toBe(false)
  })

  it('derives capability keys by stripping the endpoint prefix from a qualified id only', () => {
    expect(keysFor({ id: 'spark/qwen3-14b', qualified: true, endpoints: ['spark'] })).toEqual(['spark:qwen3-14b'])
    // A BARE id may itself contain a slash (OpenRouter names). The `qualified`
    // flag is the only thing that tells the two apart, so the prefix must not be
    // stripped here.
    expect(keysFor({ id: 'meta/llama-3.1-8b', qualified: false, endpoints: ['spark', 'local'] })).toEqual([
      'spark:meta/llama-3.1-8b',
      'local:meta/llama-3.1-8b',
    ])
  })
})

// ── The estimate ─────────────────────────────────────────────────────────────

describe('estimateRun', () => {
  it('sums a probes-only run straight off estimateProbes, skips and all', async () => {
    // Three of nine probes will skip (no vision, no tools), and `estimateProbes`
    // already zeroes their calls. This asserts `estimateRun` SUMS what it is
    // handed rather than re-deriving the skip rule — a second copy of that rule
    // is how the estimate comes to bill for calls the run never makes.
    const est = await estimateRun(
      { model: 'm', tiers: ['probes'], adversaryModel: null },
      estimateDeps({
        estimateProbes: async () =>
          probeEstimate({
            rows: [
              { id: 'json', calls: 3, promptTokens: 100, completionTokens: 20, known: false },
              { id: 'vision', calls: 0, promptTokens: 500, completionTokens: 40, known: false },
              { id: 'tools', calls: 0, promptTokens: 200, completionTokens: 30, known: false },
            ],
            calls: 3,
            promptTokens: 300,
            completionTokens: 60,
            usd: 0.0042,
          }),
      }),
    )
    expect(est.tiers).toHaveLength(1)
    expect(est.tiers[0]).toMatchObject({ tier: 'probes', calls: 3, promptTokens: 300, completionTokens: 60, usd: 0.0042, basis: 'fixture' })
    expect(est.calls).toBe(3)
    expect(est.usd).toBe(0.0042)
    expect(est.unmeasuredHarnesses).toBe(0)
  })

  it('reports a probe suite that could not size itself as zero, and voids the total', async () => {
    const est = await estimateRun(
      { model: 'm', tiers: ['probes'], adversaryModel: null },
      estimateDeps({ estimateProbes: () => Promise.reject(new Error('gateway down')) }),
    )
    expect(est.tiers[0]).toMatchObject({ calls: 0, promptTokens: 0, completionTokens: 0, usd: null })
    // A dollar figure missing a component is a number nobody can reconcile with
    // the invoice, so there is no dollar figure.
    expect(est.usd).toBeNull()
    expect(est.calls).toBe(0)
  })

  it('bills tier 2 one repair turn per JSON fixture and none for text', async () => {
    const est = await estimateRun(
      { model: 'm', tiers: ['evals'], adversaryModel: null },
      estimateDeps({ harnesses: async () => [jsonHarness('j', 4), textHarness('t', 6), textHarness('empty', 0)] }),
    )
    // 10 fixtures + 4 repairs. A registry that budgeted a repair for the six
    // text fixtures would quote 20.
    expect(est.tiers[0]?.calls).toBe(14)
    expect(est.fixtures).toBe(10)
  })

  it('prices tier 2 off the measured budget and counts the harnesses nothing has measured', async () => {
    const budget = { j: { prompt: 100, completion: 25, at: '2026-08-01T00:00:00.000Z' } }
    const est = await estimateRun(
      { model: 'm', tiers: ['evals'], adversaryModel: null },
      estimateDeps({
        harnesses: async () => [jsonHarness('j', 4), textHarness('t', 6), textHarness('empty', 0)],
        readSetting: async <T,>(key: string, fallback: T): Promise<T> => (key === 'model_fitness_budget' ? (budget as unknown as T) : fallback),
        routing: async (model) => ({ endpoints: [ep('spark', { priceInPerMtok: 1, priceOutPerMtok: 4 })], upstreamModel: model }),
      }),
    )
    expect(est.tiers[0]).toMatchObject({ promptTokens: 400, completionTokens: 100, basis: 'measured' })
    // 400 * $1/MTok + 100 * $4/MTok.
    expect(est.tiers[0]?.usd).toBeCloseTo((400 * 1 + 100 * 4) / 1e6, 12)
    // `t` has fixtures and no budget row; `empty` has no fixtures and is not a
    // gap — counting it would make the "figure is a floor" warning permanent on
    // a registry that will always have unfixtured harnesses.
    expect(est.unmeasuredHarnesses).toBe(1)
    expect(est.tiers[0]?.note).toContain('floor')
    expect(est.priced).toBe(true)
  })

  it('says nothing about a floor when every harness has been measured', async () => {
    const budget = { j: { prompt: 10, completion: 5, at: '2026-08-01T00:00:00.000Z' } }
    const est = await estimateRun(
      { model: 'm', tiers: ['evals'], adversaryModel: null },
      estimateDeps({
        harnesses: async () => [jsonHarness('j', 2), textHarness('empty', 0)],
        readSetting: async <T,>(key: string, fallback: T): Promise<T> => (key === 'model_fitness_budget' ? (budget as unknown as T) : fallback),
      }),
    )
    expect(est.unmeasuredHarnesses).toBe(0)
    expect(est.tiers[0]?.note).not.toContain('floor')
    // Nothing prices this model, so the tokens are exact and the dollars absent.
    expect(est.tiers[0]?.usd).toBeNull()
    expect(est.priced).toBe(false)
  })

  it('narrows tier 2 to the harnesses named in `only`', async () => {
    const est = await estimateRun(
      { model: 'm', tiers: ['evals'], adversaryModel: null, only: ['j'] },
      estimateDeps({ harnesses: async () => [jsonHarness('j', 3), textHarness('t', 9)] }),
    )
    expect(est.fixtures).toBe(3)
    expect(est.tiers[0]?.calls).toBe(6)
  })

  it('counts adversary calls as calls and prices both models at the dearer rate', async () => {
    let quoted: number | null = null
    const est = await estimateRun(
      { model: 'cheap', tiers: ['adversarial'], adversaryModel: 'dear' },
      estimateDeps({
        routing: async (model) => ({
          endpoints: [model === 'dear' ? ep('vendor', { priceInPerMtok: 10, priceOutPerMtok: 30 }) : ep('local', { priceInPerMtok: 1, priceOutPerMtok: 2 })],
          upstreamModel: model,
        }),
        estimateAdversarial: async (opts) => {
          quoted = opts.price ? await opts.price(1_000_000, 0) : null
          return adversarialEstimate({ calls: 12, adversaryCalls: 12, promptTokens: 5_000, completionTokens: 2_000, costUsd: 0.9, worstCase: true })
        },
      }),
    )
    // Candidate + adversary, both counted: the run pays for both.
    expect(est.tiers[0]?.calls).toBe(24)
    // Priced at the DEAR model's rate — a ceiling, never a surprise upward.
    expect(quoted).toBeCloseTo(10, 12)
    expect(est.tiers[0]?.note).toContain('ceiling')
  })

  it('drops the escalation round from the call count when no adversary is named', async () => {
    const est = await estimateRun(
      { model: 'm', tiers: ['adversarial'], adversaryModel: null },
      estimateDeps({ estimateAdversarial: async () => adversarialEstimate({ calls: 12, adversaryCalls: 0, costUsd: 0.1 }) }),
    )
    expect(est.tiers[0]?.calls).toBe(12)
    expect(est.tiers[0]?.note).toContain('Naming an adversary')
  })

  it('adds up three tiers, and voids the total when one of them cannot be priced', async () => {
    const budget = { j: { prompt: 100, completion: 20, at: '2026-08-01T00:00:00.000Z' } }
    const deps = estimateDeps({
      harnesses: async () => [jsonHarness('j', 2)],
      readSetting: async <T,>(key: string, fallback: T): Promise<T> => (key === 'model_fitness_budget' ? (budget as unknown as T) : fallback),
      routing: async (model) => ({ endpoints: [ep('spark', { priceInPerMtok: 2, priceOutPerMtok: 2 })], upstreamModel: model }),
      estimateProbes: async () => probeEstimate({ calls: 9, promptTokens: 900, completionTokens: 90, usd: 0.001 }),
      estimateAdversarial: async () => adversarialEstimate({ calls: 12, adversaryCalls: 0, costUsd: 0.02 }),
    })

    const priced = await estimateRun({ model: 'm', tiers: ['probes', 'evals', 'adversarial'], adversaryModel: null }, deps)
    expect(priced.tiers.map((t) => t.tier)).toEqual(['probes', 'evals', 'adversarial'])
    // 9 probe calls + (2 fixtures + 2 repairs) + 12 provocations.
    expect(priced.calls).toBe(25)
    const evalsUsd = (200 * 2 + 40 * 2) / 1e6
    expect(priced.usd).toBeCloseTo(0.001 + evalsUsd + 0.02, 12)

    const unpriced = await estimateRun(
      { model: 'm', tiers: ['probes', 'evals', 'adversarial'], adversaryModel: null },
      { ...deps, estimateAdversarial: async () => adversarialEstimate({ calls: 12, costUsd: null }) },
    )
    expect(unpriced.calls).toBe(25)
    expect(unpriced.usd).toBeNull()
  })

  it('emits no row for a tier that was not asked for', async () => {
    const est = await estimateRun(
      { model: 'm', tiers: ['evals'], adversaryModel: null },
      estimateDeps({
        harnesses: async () => [jsonHarness('j', 1)],
        estimateProbes: () => Promise.reject(new Error('never called')),
        estimateAdversarial: () => Promise.reject(new Error('never called')),
      }),
    )
    expect(est.tiers.map((t) => t.tier)).toEqual(['evals'])
  })
})

describe('priceOf', () => {
  it('takes the dearest endpoint in the pool, not the average', async () => {
    const price = await priceOf('m', {
      routing: async (model) => ({
        endpoints: [ep('a', { priceInPerMtok: 1, priceOutPerMtok: 1 }), ep('b', { priceInPerMtok: 5, priceOutPerMtok: 9 })],
        upstreamModel: model,
      }),
    })
    // An estimate the round-robin can exceed is not an estimate an admin can act on.
    expect(price).toEqual({ in: 5, out: 9 })
  })

  it('prefers the admin override over the auto-fetched catalog rate', async () => {
    const price = await priceOf('m', {
      routing: async () => ({
        endpoints: [ep('a', { modelPrices: { up: { in: 3, out: 7 } }, autoPrices: { up: { in: 1, out: 1 } }, priceInPerMtok: 99, priceOutPerMtok: 99 })],
        upstreamModel: 'up',
      }),
    })
    expect(price).toEqual({ in: 3, out: 7 })
  })

  it('is null when nothing serves or prices the model', async () => {
    expect(await priceOf('m', { routing: async (model) => ({ endpoints: [], upstreamModel: model }) })).toBeNull()
    expect(await priceOf('m', { routing: async (model) => ({ endpoints: [ep('a')], upstreamModel: model }) })).toBeNull()
    expect(await priceOf('m', { routing: () => Promise.reject(new Error('no catalog')) })).toBeNull()
  })

  it('gives no dollars without a price rather than quoting zero', () => {
    expect(usdOf(null, 1_000_000, 1_000_000)).toBeNull()
    expect(usdOf({ in: 2, out: 6 }, 1_000_000, 500_000)).toBeCloseTo(5, 12)
  })
})

// ── The drill-down ───────────────────────────────────────────────────────────

describe('drilldown', () => {
  const heavy = (n: number): EvalCaseScore =>
    evalCase({ case: `fail-${n}`, contractHeld: false, task: 'fail', prompt: `prompt ${n}`, raw: `reply ${n}`, error: 'schema rejected' })
  const clean = (n: number): EvalCaseScore => evalCase({ case: `pass-${n}` })

  it('keeps the stored prompt and reply verbatim — that is what makes a red cell trustworthy', () => {
    const { kept, dropped } = drilldown([clean(1), heavy(1), clean(2)])
    expect(dropped).toBe(0)
    const failed = kept.find((c) => c.case === 'fail-1')
    expect(failed?.prompt).toBe('prompt 1')
    expect(failed?.raw).toBe('reply 1')
    expect(failed?.error).toBe('schema rejected')
  })

  it('keeps a case that carries only one half of the transcript', () => {
    // `run.ts` can record a prompt with no reply (the transport died) or a reply
    // with no prompt. Either is a transcript and either is heavy.
    const promptOnly = evalCase({ case: 'p', prompt: 'asked', raw: null })
    const replyOnly = evalCase({ case: 'r', prompt: null, raw: 'answered' })
    const { kept } = drilldown([promptOnly, replyOnly])
    expect(kept.map((c) => c.case)).toEqual(['p', 'r'])
  })

  it('keeps every clean case whole even past the cap — the cap is on transcripts', () => {
    const cases = [...Array.from({ length: 40 }, (_, i) => clean(i)), ...Array.from({ length: 5 }, (_, i) => heavy(i))]
    const { kept, dropped } = drilldown(cases, 3)
    expect(dropped).toBe(2)
    // 3 transcripts + all 40 clean rows: dropping the cheap ones would leave the
    // panel unable to say how many fixtures actually passed.
    expect(kept).toHaveLength(43)
    expect(kept.filter((c) => c.prompt !== null || c.raw !== null)).toHaveLength(3)
  })

  it('drops nothing at exactly the cap and one at the cap plus one', () => {
    const atCap = Array.from({ length: 30 }, (_, i) => heavy(i))
    expect(drilldown(atCap).dropped).toBe(0)
    expect(drilldown(atCap).kept).toHaveLength(30)

    const overCap = [...atCap, heavy(30)]
    const over = drilldown(overCap)
    expect(over.dropped).toBe(1)
    expect(over.kept).toHaveLength(30)
    // The newest transcript is the one that falls off — the slice keeps the
    // head, which is sweep order.
    expect(over.kept.map((c) => c.case)).not.toContain('fail-30')
  })

  it('reports no drop for an empty sweep', () => {
    expect(drilldown([])).toEqual({ kept: [], dropped: 0 })
  })
})

// ── The archive ──────────────────────────────────────────────────────────────

describe('evictArchive', () => {
  const indexOf = (n: number): FitnessIndex => {
    const out: FitnessIndex = {}
    // Model 0 is the OLDEST. Dates are ISO so the sort is lexicographic.
    for (let i = 0; i < n; i++) out[`m${i}`] = entry(`m${i}`, `2026-08-${String(i + 1).padStart(2, '0')}T00:00:00.000Z`)
    return out
  }

  it('evicts nothing below the cap', () => {
    const { index, evicted } = evictArchive(indexOf(23), 24)
    expect(evicted).toEqual([])
    expect(Object.keys(index)).toHaveLength(23)
  })

  it('evicts nothing at exactly the cap', () => {
    const { index, evicted } = evictArchive(indexOf(24), 24)
    expect(evicted).toEqual([])
    expect(Object.keys(index)).toHaveLength(24)
  })

  it('evicts the single oldest at the cap plus one', () => {
    const { index, evicted } = evictArchive(indexOf(25), 24)
    expect(evicted).toEqual(['m0'])
    expect(Object.keys(index)).toHaveLength(24)
    expect(index.m0).toBeUndefined()
    expect(index.m24).toBeDefined()
  })

  it('evicts oldest-first when an old index is well over the cap', () => {
    const { index, evicted } = evictArchive(indexOf(30), 24)
    expect(evicted).toEqual(['m5', 'm4', 'm3', 'm2', 'm1', 'm0'])
    expect(Object.keys(index)).toHaveLength(24)
  })

  it('leaves the index it was handed untouched', () => {
    const before = indexOf(26)
    evictArchive(before, 24)
    // The index and the report rows are written together; a mutation that
    // landed before a failed write would leave the matrix listing models whose
    // reports had already been deleted.
    expect(Object.keys(before)).toHaveLength(26)
  })

  it('defaults to the shipped cap', () => {
    expect(evictArchive(indexOf(25)).evicted).toEqual(['m0'])
  })
})

// ── The write verbs ──────────────────────────────────────────────────────────

describe('startFitnessRun rejections', () => {
  // Every one of these returns BEFORE the run slot is claimed and before a
  // single model call is bought, so none of them starts anything.
  const deps: Partial<SurfaceDeps> = {
    models: async () => [
      { id: 'spark/qwen3-14b', endpoints: ['spark'], qualified: true },
      { id: 'spark/gpt-5', endpoints: ['spark'], qualified: true },
    ],
    capabilities: async () => ({}),
  }
  const req = { tiers: ['probes'] as const, restart: false }

  it('refuses a candidate the gateway does not serve', async () => {
    const out = await startFitnessRun({ ...req, model: 'not-a-model', tiers: ['probes'], adversaryModel: null }, deps)
    expect(out).toEqual({ ok: false, reason: 'rejected', error: 'that model is not on the gateway' })
  })

  it('refuses an adversary the gateway does not serve', async () => {
    const out = await startFitnessRun({ ...req, model: 'spark/qwen3-14b', tiers: ['probes'], adversaryModel: 'ghost' }, deps)
    expect(out).toMatchObject({ ok: false, reason: 'rejected', error: 'that adversary model is not on the gateway' })
  })

  it('refuses to let a model grade its own resistance', async () => {
    // The who-judges-the-judge regress with the stakes turned up: a model asked
    // to break itself scores itself safe, and the number goes on the page.
    const out = await startFitnessRun({ ...req, model: 'spark/gpt-5', tiers: ['adversarial'], adversaryModel: 'spark/gpt-5' }, deps)
    expect(out).toMatchObject({ ok: false, reason: 'rejected', error: 'the adversary must be a different model than the candidate' })
  })
})

describe('forgetModel', () => {
  const deps = (forgotten: string[], store: Record<string, unknown> = {}): Partial<SurfaceDeps> => ({
    models: async () => [{ id: 'qwen3-14b', endpoints: ['spark', 'local'], qualified: false }],
    capabilities: async () => ({}),
    forget: async (key) => {
      forgotten.push(key)
    },
    readSetting: async <T,>(key: string, fallback: T): Promise<T> => (key in store ? (store[key] as T) : fallback),
    writeSetting: async (key, value) => {
      store[key] = value
    },
  })

  it('clears every endpoint key a bare id could be served from', async () => {
    // Per endpoint:model rather than per id — the release valve on the
    // gateway's one-way ratchet has to reach every fact the id has.
    const forgotten: string[] = []
    const out = await forgetModel('qwen3-14b', deps(forgotten))
    expect(out.ok).toBe(true)
    expect(forgotten).toEqual(['spark:qwen3-14b', 'local:qwen3-14b'])
    expect(out.ok && out.keys).toEqual(['spark:qwen3-14b', 'local:qwen3-14b'])
  })

  it('deletes the archived report and its index entry, not only the capability facts', async () => {
    // THE REASON THE BUTTON LOOKED BROKEN. Talaria records what it knows about
    // a model in two places and this cleared one, so an admin pressed Forget,
    // the panel refetched, and every probe verdict they had just been told was
    // deleted was still on the screen.
    const store: Record<string, unknown> = {
      [recordKey('qwen3-14b')]: { model: 'qwen3-14b' },
      [INDEX_KEY]: { 'qwen3-14b': { model: 'qwen3-14b', at: 'x' }, 'other-model': { model: 'other-model', at: 'y' } },
    }
    const out = await forgetModel('qwen3-14b', deps([], store))

    expect(out).toMatchObject({ ok: true, report: true })
    expect(store[recordKey('qwen3-14b')]).toBeNull()
    // Only this model leaves the index; every other verdict on the page stays.
    expect(Object.keys(store[INDEX_KEY] as object)).toEqual(['other-model'])
  })

  it('is idempotent on a model nobody has swept', async () => {
    const forgotten: string[] = []
    const out = await forgetModel('qwen3-14b', deps(forgotten))
    // The facts still go; there was simply no report to go with them, which is
    // a thing to report rather than a thing to fail on.
    expect(out).toMatchObject({ ok: true, report: false })
    expect(forgotten).toEqual(['spark:qwen3-14b', 'local:qwen3-14b'])
  })

  it('refuses an id the gateway does not serve, and forgets nothing', async () => {
    const forgotten: string[] = []
    expect(await forgetModel('ghost', deps(forgotten))).toEqual({ ok: false, error: 'that model is not on the gateway' })
    expect(forgotten).toEqual([])
  })
})

describe('indexEntryOf', () => {
  const report: FitnessReport = { model: 'm', slots: [], unbound: [], guarded: true }
  const sweep = (over: Partial<EvalSweep> = {}): EvalSweep => ({
    model: 'm',
    state: 'done',
    startedAt: '2026-08-01T00:00:00.000Z',
    finishedAt: '2026-08-01T00:01:00.000Z',
    done: 10,
    total: 10,
    error: null,
    concurrency: { requested: 1, ended: 1, low: 1, narrowedBecause: null },
    measured: [],
    harnesses: [],
    cases: [],
    unfixtured: [],
    guarded: true,
    ...over,
  })
  const probes = (over: Partial<ProbeReport> = {}): ProbeReport => ({
    model: 'm',
    keys: [],
    results: [],
    wrote: 0,
    latency: { requests: 0, errors: 0, p50: 0, p95: 0, usd: null },
    ambiguous: null,
    ...over,
  })

  const parts = { model: 'm', at: '2026-08-06T00:00:00.000Z', report, adversarial: null }

  it('is not partial when every requested tier produced a finished sweep', () => {
    const e = indexEntryOf({ ...parts, ran: ['probes', 'evals'], requested: ['probes', 'evals'], sweep: sweep(), probes: probes({ wrote: 4 }) })
    expect(e.partial).toBe(false)
    expect(e.probesWrote).toBe(4)
  })

  it('is partial when a requested tier produced nothing', () => {
    // The archived record is stamped with the tiers that RAN. A record claiming
    // a tier that never happened is the same lie as a green cell nobody filled.
    const e = indexEntryOf({ ...parts, ran: ['evals'], requested: ['probes', 'evals'], sweep: sweep(), probes: null })
    expect(e.partial).toBe(true)
    expect(e.tiers).toEqual(['evals'])
    expect(e.probesWrote).toBe(0)
  })

  it('is partial when the sweep stopped mid-run', () => {
    expect(indexEntryOf({ ...parts, ran: ['evals'], requested: ['evals'], sweep: sweep({ state: 'stopped', done: 3 }), probes: null }).partial).toBe(true)
  })
  const hs = (over: Partial<HarnessScore>): HarnessScore =>
    ({ id: 'h', label: 'H', source: 'builtin', cases: 4, skipped: 0, gaps: 0, gapReasons: [], skipReason: null, scored: 4, promptTokens: 0, completionTokens: 0, costUsd: null, ...over }) as HarnessScore

  it('does not call a fully-priced run "unpriced" because one harness was skipped', () => {
    // WHAT MADE THIS SHOW UP. A harness whose cases were all skipped reports
    // `costUsd: null` — it priced nothing because it SPENT nothing. Treating
    // that as "unpriced" turned every run with a skip into a dash in the modal
    // header, and the qwen run (a routing refusal skipped everything) made it
    // impossible to miss.
    const e = indexEntryOf({
      ...parts,
      ran: ['evals'],
      requested: ['evals'],
      probes: null,
      sweep: sweep({
        harnesses: [
          hs({ id: 'a', costUsd: 0.02, promptTokens: 900, completionTokens: 120 }),
          hs({ id: 'b', costUsd: 0.01, promptTokens: 400, completionTokens: 80 }),
          hs({ id: 'c', costUsd: null, promptTokens: 0, completionTokens: 0, cases: 0, skipped: 3 }),
        ],
      }),
    })
    expect(e.costUsd).toBeCloseTo(0.03)
  })

  it('still refuses a total when something that BURNED tokens could not be priced', () => {
    // The case the all-or-nothing rule was written for: a partial total under a
    // dollar sign is a number nobody can reconcile with the invoice.
    const e = indexEntryOf({
      ...parts,
      ran: ['evals'],
      requested: ['evals'],
      probes: null,
      sweep: sweep({
        harnesses: [hs({ id: 'a', costUsd: 0.02, promptTokens: 900, completionTokens: 120 }), hs({ id: 'b', costUsd: null, promptTokens: 400, completionTokens: 80 })],
      }),
    })
    expect(e.costUsd).toBeNull()
  })

})

describe('the token budget a sweep leaves behind', () => {
  const score = (id: string, over: Partial<HarnessScore> = {}): HarnessScore =>
    ({ id, cases: 4, promptTokens: 4000, completionTokens: 800, ...over }) as unknown as HarnessScore

  const good: TokenBudget = { titler: { prompt: 103, completion: 11, at: 'yesterday' } }

  it('records per-case tokens for what a run actually measured', () => {
    expect(nextBudget({}, [score('titler')], 'now')).toEqual({ titler: { prompt: 1000, completion: 200, at: 'now' } })
  })

  it('does not let a run that measured nothing overwrite one that did', () => {
    // THE BUG THIS LOCKS. A sweep against a model id the gateway could not
    // reach ran every case, failed every one before a token moved, and wrote
    // 0/0 across the registry. Both dollar figures downstream then read $0.00
    // for every model — a confident number nobody could reconcile.
    const after = nextBudget(good, [score('titler', { promptTokens: 0, completionTokens: 0 })], 'now')

    expect(after).toEqual(good)
  })

  it('still skips a harness the sweep never ran a case of', () => {
    expect(nextBudget(good, [score('titler', { cases: 0, promptTokens: 0, completionTokens: 0 })], 'now')).toEqual(good)
  })

  it('takes the measured harnesses and leaves the unmeasured ones alone', () => {
    const after = nextBudget(good, [score('titler', { promptTokens: 0, completionTokens: 0 }), score('summarizer')], 'now')

    expect(after.titler).toEqual(good.titler)
    expect(after.summarizer).toEqual({ prompt: 1000, completion: 200, at: 'now' })
  })
})

describe('the archive, re-keyed onto the ids the catalog offers', () => {
  const catalog = [
    { id: 'openrouter/deepseek/deepseek-v4-flash', endpoints: ['openrouter'], qualified: true },
    { id: 'spark-a/qwen3-14b', endpoints: ['spark-a'], qualified: true },
    { id: 'spark-b/qwen3-14b', endpoints: ['spark-b'], qualified: true },
    { id: 'qwen3-14b', endpoints: ['spark-a', 'spark-b'], qualified: false },
  ]
  const at = (model: string, when: string): FitnessIndexEntry =>
    ({ model, at: when, tiers: ['evals'], guarded: true, cells: {}, safety: null, probesWrote: 0, speed: null, costUsd: null, calls: 0, partial: false })

  it('lights the canonical row from a report archived under the bare id', () => {
    // The run was paid for. Left keyed bare, its verdicts colour no cell and
    // the page asks the admin to buy it again.
    const out = canonicalIndex({ 'deepseek/deepseek-v4-flash': at('deepseek/deepseek-v4-flash', 'monday') }, catalog)

    expect(Object.keys(out)).toEqual(['openrouter/deepseek/deepseek-v4-flash'])
    // THE KEY MOVES, THE STORED SPELLING DOES NOT. `model` is what
    // `recordKey` needs, and overwriting it with the canonical id is what
    // broke the drill-down and the value view's backfill: both went looking
    // for `model_fitness_report:openrouter/deepseek/…` when the archive is
    // filed under `model_fitness_report:deepseek/…`.
    expect(out['openrouter/deepseek/deepseek-v4-flash']?.model).toBe('deepseek/deepseek-v4-flash')
    expect(storedIdFor('openrouter/deepseek/deepseek-v4-flash', out)).toBe('deepseek/deepseek-v4-flash')
  })

  it('leaves an id that never moved alone', () => {
    const out = canonicalIndex({ 'spark-a/qwen3-14b': at('spark-a/qwen3-14b', 'monday') }, catalog)

    expect(storedIdFor('spark-a/qwen3-14b', out)).toBe('spark-a/qwen3-14b')
    // And a model with no entry at all is its own stored id.
    expect(storedIdFor('never-tested', out)).toBe('never-tested')
  })

  it('never displaces an entry already stored under its canonical id', () => {
    const out = canonicalIndex(
      {
        'deepseek/deepseek-v4-flash': at('deepseek/deepseek-v4-flash', 'monday'),
        'openrouter/deepseek/deepseek-v4-flash': at('openrouter/deepseek/deepseek-v4-flash', 'friday'),
      },
      catalog,
    )

    expect(out['openrouter/deepseek/deepseek-v4-flash']?.at).toBe('friday')
  })

  it('leaves a pooled id where it is', () => {
    // `qwen3-14b` is a round-robin target in its own right, not a misspelling
    // of either endpoint's pin.
    const out = canonicalIndex({ 'qwen3-14b': at('qwen3-14b', 'monday') }, catalog)

    expect(Object.keys(out)).toEqual(['qwen3-14b'])
  })
})

describe('testing several candidates at once', () => {
  // A REAL SLOT IS CLAIMED by every start that gets past validation, and this
  // process has no way to un-start one — so each case here stops what it
  // started. The tiers are stubbed to resolve immediately, so no model call is
  // ever bought.
  const store = (): Record<string, unknown> => ({})
  const deps = (settings: Record<string, unknown>): Partial<SurfaceDeps> => ({
    models: async () => [
      ...Array.from({ length: MAX_CONCURRENT_RUNS }, (_, i) => ({ id: `spark/m${i}`, endpoints: ['spark'], qualified: true })),
      { id: 'spark/over', endpoints: ['spark'], qualified: true },
      { id: 'spark/a', endpoints: ['spark'], qualified: true },
    ],
    capabilities: async () => ({}),
    readSetting: async <T,>(key: string, fallback: T): Promise<T> => (key in settings ? (settings[key] as T) : fallback),
    writeSetting: async (key, value) => {
      settings[key] = value
    },
    evalSweepStatuses: async () => ({}),
    stopEvalSweep: () => false,
    // Nothing runs: every tier is stubbed, so the slot is claimed and released
    // without a call. `runProbes` is the only tier these starts ask for.
    runProbes: async () => ({ results: [], wrote: 0 }) as never,
    estimateProbes: async () => ({ calls: 0, promptTokens: 0, completionTokens: 0, usd: null }) as never,
  })

  const start = (model: string, d: Partial<SurfaceDeps>) =>
    startFitnessRun({ model, tiers: ['probes'], adversaryModel: null, restart: false }, d)

  it('runs candidates side by side up to the cap, and refuses the one past it', async () => {
    // THE RUNS ARE HELD OPEN ON PURPOSE. Every tier is stubbed, so without a gate
    // a detached run finishes before the next Start is issued and releases its
    // slot — the cap would never be reached and the test would pass for the
    // wrong reason at 3 and silently stop testing anything at 8.
    let release!: () => void
    const gate = new Promise<void>((r) => (release = r))
    const settings = store()
    const d = { ...deps(settings), runProbes: (async () => { await gate; return { results: [], wrote: 0 } }) as never }
    const ids = Array.from({ length: MAX_CONCURRENT_RUNS }, (_, i) => `spark/m${i}`)
    try {
      for (const m of ids) expect((await start(m, d)).ok, m).toBe(true)

      const overflow = await start('spark/over', d)
      expect(overflow).toMatchObject({ ok: false, reason: 'busy', refusal: 'at-capacity' })
      expect((await fitnessRuns(d)).full).toBe(true)
    } finally {
      release()
      await stopFitnessRun(null, d)
      // DRAIN ON THE REAL SIGNAL, not on a guessed number of microtasks. The run
      // slots are module state shared by every case in this file, so one that
      // leaks turns the next case's `already-running` into `at-capacity`.
      for (let i = 0; i < 200 && runningModels().length > 0; i++) await new Promise((r) => setTimeout(r, 1))
    }
  })

  it('names the refusal so the route can say WHICH wall was hit', async () => {
    // "busy" alone had one meaning when one run was the maximum. With three
    // slots, "you already started this one" and "every slot is taken" are
    // different sentences and different fixes.
    const settings = store()
    const d = deps(settings)
    try {
      await start('spark/a', d)
      expect(await start('spark/a', d)).toMatchObject({ refusal: 'already-running' })
    } finally {
      await stopFitnessRun(null, d)
    }
  })

  it('reports the cap so the page never has to restate it', async () => {
    const settings = store()
    const d = deps(settings)
    const view = await fitnessRuns(d)

    expect(view.max).toBe(MAX_CONCURRENT_RUNS)
    // Raised from 3 once the checkpoint stopped being one shared row — see the
    // note on the constant. Held here so a change is deliberate.
    expect(MAX_CONCURRENT_RUNS).toBe(8)
  })

  it('folds the legacy single status row in under its own model', async () => {
    // The row a run already in flight keeps writing across this change. Not
    // reading it means an admin watches a run start and never sees it finish.
    const legacy: FitnessRunStatus = { state: 'running', model: 'spark/legacy', tiers: ['evals'], phase: 'evals', startedAt: 'monday' }
    const view = await fitnessRuns(deps({ model_fitness_status: legacy }))

    expect(view.runs.map((r) => r.model)).toEqual(['spark/legacy'])
  })

  it('lets a real entry win over the legacy row for the same model', async () => {
    const legacy: FitnessRunStatus = { state: 'running', model: 'spark/a', tiers: ['evals'], phase: 'evals', startedAt: 'monday' }
    const current: FitnessRunStatus = { state: 'done', model: 'spark/a', tiers: ['evals'], phase: null, startedAt: 'friday' }
    const view = await fitnessRuns(deps({ model_fitness_status: legacy, model_fitness_runs: { 'spark/a': current } }))

    expect(view.runs).toHaveLength(1)
    expect(view.runs[0]?.state).toBe('done')
  })

  it('puts the running rows first, then the most recently started', async () => {
    // `heartbeatAt: now` on the live row: this test is about ORDERING, and
    // without a heartbeat a 2026-01-02 row is (correctly) months stale and
    // reports as failed. See `staleRun`.
    const row = (model: string, state: FitnessRunStatus['state'], startedAt: string): FitnessRunStatus => ({
      state,
      model,
      tiers: ['evals'],
      phase: null,
      startedAt,
      ...(state === 'running' ? { heartbeatAt: new Date().toISOString() } : {}),
    })
    const view = await fitnessRuns(
      deps({
        model_fitness_runs: {
          old: row('old', 'done', '2026-01-01'),
          live: row('live', 'running', '2026-01-02'),
          recent: row('recent', 'done', '2026-01-03'),
        },
      }),
    )

    expect(view.runs.map((r) => r.model)).toEqual(['live', 'recent', 'old'])
  })
})

describe('the live case list', () => {
  const c = (over: Partial<EvalCaseScore>): EvalCaseScore =>
    ({ harness: 'h', case: 'x', band: 'standard', skipped: null, contractHeld: true, firstPass: true, task: 'pass', timedOut: false, ...over }) as EvalCaseScore

  it('keeps everything that failed, and only a few of what passed', () => {
    // Polled every three seconds. A clean case says nothing the progress counter
    // does not already say, and sending 155 of them per poll — growing to 247 —
    // is the whole sweep on the wire twenty times a minute.
    const cases = [...Array.from({ length: 100 }, () => c({})), c({ task: 'fail' }), c({ contractHeld: false }), c({ timedOut: true })]
    const out = liveCases(cases)

    expect(out.kept.filter((x) => x.task === 'fail' || !x.contractHeld || x.timedOut)).toHaveLength(3)
    expect(out.kept.length).toBeLessThan(12)
    expect(out.dropped).toBeGreaterThan(90)
  })

  it('never counts a SKIPPED case as a failure', () => {
    // A skip is an absence, not a zero — the distinction the whole scoring layer
    // turns on, and it must not be undone by the view that shows it. A skipped
    // case carries `contractHeld: false`, so a filter that forgot to check
    // `skipped` would retain every one of them as a failure forever.
    const skip = c({ skipped: 'this candidate runs no tool loop', contractHeld: false, task: 'unscored' })
    const out = liveCases([skip, ...Array.from({ length: 20 }, () => c({}))])

    expect(out.kept.filter((x) => x.skipped !== null)).toHaveLength(0)
  })
})

describe('the Stop button', () => {
  const settingsWith = (over: Record<string, unknown>): Record<string, unknown> => ({ ...over })
  const d = (settings: Record<string, unknown>, over: Partial<SurfaceDeps> = {}): Partial<SurfaceDeps> => ({
    readSetting: async <T,>(key: string, fallback: T): Promise<T> => (key in settings ? (settings[key] as T) : fallback),
    writeSetting: async (key, value) => {
      settings[key] = value
    },
    evalSweepStatuses: async () => ({}),
    stopEvalSweep: () => false,
    models: async () => [],
    ...over,
  })

  const running = (model: string): FitnessRunStatus => ({ state: 'running', model, tiers: ['evals'], phase: 'evals', startedAt: 'now' })

  it('stops a run THIS INSTANCE DOES NOT HOLD, which is the bug', () => {
    // The request used to be a boolean on an in-process map, so it could only
    // reach a run whose closure lived in the module instance the request
    // happened to hit. One HMR reload — or one restart, or a second process —
    // and Stop returned `stopped: false` while the sweep carried on for another
    // twenty minutes. Observed exactly that, three sweeps at once.
    const settings = settingsWith({ model_fitness_runs: { 'spark/a': running('spark/a') } })

    return stopFitnessRun('spark/a', d(settings)).then(async (out) => {
      expect(out.stopped).toBe(true)
      expect(settings.model_fitness_stop).toEqual(['spark/a'])
      // And the sweep, wherever it is running, can see it.
      expect(await stopRequestedFor('spark/a', d(settings))).toBe(true)
    })
  })

  it('stops every live run when asked for all of them', async () => {
    const settings = settingsWith({ model_fitness_runs: { 'spark/a': running('spark/a'), 'spark/b': running('spark/b') } })
    const out = await stopFitnessRun(null, d(settings))

    expect(out.stopped).toBe(true)
    expect((settings.model_fitness_stop as string[]).sort()).toEqual(['spark/a', 'spark/b'])
  })

  it('reports honestly when there is nothing running to stop', async () => {
    const out = await stopFitnessRun(null, d(settingsWith({})))
    expect(out.stopped).toBe(false)
  })

  // ── ORPHANS ────────────────────────────────────────────────────────────────
  //
  // A run is a promise inside a process; its status is a row in the database.
  // A restart separates the two, and the row went on claiming `running` for
  // ever: the panel counted it against the concurrency limit, `full` went true,
  // and Stop wrote a request that no living thing would ever read. Two of them
  // accumulated in one afternoon of restarts, which is how this was found.
  const HOUR_AGO = new Date(Date.parse('2026-01-01T12:00:00.000Z') - 60 * 60_000).toISOString()
  const NOW = '2026-01-01T12:00:00.000Z'
  const stale = (model: string): FitnessRunStatus => ({
    state: 'running',
    model,
    tiers: ['evals'],
    phase: 'evals',
    startedAt: HOUR_AGO,
    heartbeatAt: HOUR_AGO,
  })
  const fresh = (model: string): FitnessRunStatus => ({ ...stale(model), heartbeatAt: NOW })

  it('reports a run that stopped breathing as FAILED, not as running', async () => {
    const out = await fitnessRuns(d(settingsWith({ model_fitness_runs: { 'spark/dead': stale('spark/dead') } }), { nowIso: () => NOW }))
    const row = out.runs.find((r) => r.model === 'spark/dead')!
    expect(row.state).toBe('error')
    expect(row.error).toMatch(/interrupted/)
    // And it stops holding a concurrency slot, which is what made the panel
    // refuse to start anything after two restarts.
    expect(out.full).toBe(false)
  })

  it('leaves a run that is still breathing alone', async () => {
    const out = await fitnessRuns(d(settingsWith({ model_fitness_runs: { 'spark/live': fresh('spark/live') } }), { nowIso: () => NOW }))
    expect(out.runs.find((r) => r.model === 'spark/live')!.state).toBe('running')
  })

  it('does not REWRITE the row on a read', async () => {
    // A read is not the place to take a durable decision: two instances reading
    // at once would both write, and a merely slow run would have its row
    // destroyed by whoever looked first. Reporting is enough.
    const settings = settingsWith({ model_fitness_runs: { 'spark/dead': stale('spark/dead') } })
    await fitnessRuns(d(settings, { nowIso: () => NOW }))
    expect((settings.model_fitness_runs as Record<string, FitnessRunStatus>)['spark/dead']!.state).toBe('running')
  })

  it('CLEARS an orphan when Stop is pressed, rather than leaving a note nobody reads', async () => {
    const settings = settingsWith({ model_fitness_runs: { 'spark/dead': stale('spark/dead') } })
    const out = await stopFitnessRun('spark/dead', d(settings, { nowIso: () => NOW }))

    expect(out.stopped).toBe(true)
    const row = (settings.model_fitness_runs as Record<string, FitnessRunStatus>)['spark/dead']!
    expect(row.state).toBe('error')
    expect(row.finishedAt).toBe(NOW)
    // The note is pointless now, and left behind it would stop the NEXT run on
    // this model after one case — a bug this file has already had once.
    expect(settings.model_fitness_stop ?? []).toEqual([])
  })

  it('clears an orphan on Stop-ALL, which is how the panel button calls it', async () => {
    // THE CASE THE NAMED-MODEL TEST ABOVE CANNOT REACH, and it was broken for
    // exactly one commit. Stop-all builds its target list by asking which runs
    // are live — and once the read started reporting a stale row as `error` to
    // keep it off the panel, that list no longer contained the orphan. Stop
    // returned `stopped: false` while the row went on saying `running` for ever,
    // which is the original bug wearing the fix's clothes.
    const settings = settingsWith({ model_fitness_runs: { 'spark/dead': stale('spark/dead') } })
    const out = await stopFitnessRun(null, d(settings, { nowIso: () => NOW }))

    expect(out.stopped).toBe(true)
    expect((settings.model_fitness_runs as Record<string, FitnessRunStatus>)['spark/dead']!.state).toBe('error')
    expect(settings.model_fitness_stop ?? []).toEqual([])
  })

  it('does NOT clear a live run belonging to another instance', async () => {
    // Killing that row from here would report a sweep as failed while it was
    // still spending money. The heartbeat is what tells the two apart.
    const settings = settingsWith({ model_fitness_runs: { 'spark/live': fresh('spark/live') } })
    await stopFitnessRun('spark/live', d(settings, { nowIso: () => NOW }))

    expect((settings.model_fitness_runs as Record<string, FitnessRunStatus>)['spark/live']!.state).toBe('running')
    // It gets the ordinary request instead — the run reads it between cases.
    expect(settings.model_fitness_stop).toEqual(['spark/live'])
  })
})

// ── The live feed ────────────────────────────────────────────────────────────

describe('liveLog', () => {
  const line = (over: Partial<EvalCaseScore>): EvalCaseScore => evalCase(over)

  it('classifies each case into the vocabulary the terminal colours by', () => {
    const out = liveLog([
      line({ case: 'ok' }),
      line({ case: 'checked', task: 'fail', taskError: 'wrong shape' }),
      line({ case: 'ours', gap: 'the fixture never gave it the id' }),
      line({ case: 'never', skipped: 'no tool loop on this candidate' }),
      line({ case: 'slow', timedOut: true }),
      line({ case: 'broke', contractHeld: false, error: 'gateway completion 429' }),
    ])
    expect(out.map((l) => l.verdict)).toEqual(['pass', 'fail', 'gap', 'skip', 'timeout', 'error'])
  })

  it('puts a SKIP and a TIMEOUT ahead of the contract flags they both carry', () => {
    // Both land with `contractHeld: false` — the same zero every unmeasured
    // field carries. Reading either as an error is the exact mistake the
    // separate verdicts exist to prevent.
    expect(liveLog([line({ skipped: 'no tool loop', contractHeld: false })])[0]?.verdict).toBe('skip')
    expect(liveLog([line({ timedOut: true, contractHeld: false })])[0]?.verdict).toBe('timeout')
  })

  it('carries the reason, error first, because an error is the cause of the failure beside it', () => {
    const [l] = liveLog([line({ contractHeld: false, error: 'gateway completion 429: rate limited', task: 'fail', taskError: 'no value to grade' })])
    expect(l?.note).toContain('429')
  })

  it('is bounded, and keeps the NEWEST lines — a log reads downward', () => {
    const many = Array.from({ length: LIVE_LOG_CAP + 25 }, (_, i) => line({ case: `c${i}` }))
    const out = liveLog(many)
    expect(out).toHaveLength(LIVE_LOG_CAP)
    expect(out.at(-1)?.case).toBe(`c${LIVE_LOG_CAP + 24}`)
  })

  it('stays small enough to poll every three seconds', () => {
    // The whole point of a separate feed: `liveCases` cannot ship 250 full
    // transcripts, and a failures-only list cannot show a sweep moving. A line
    // is roughly ninety bytes, so an entire sweep is tens of kilobytes.
    const sweep = Array.from({ length: 250 }, (_, i) => line({ harness: 'work-session', case: `fixture number ${i}`, latencyMs: 900 + i }))
    expect(JSON.stringify(liveLog(sweep)).length).toBeLessThan(60_000)
  })
})


// ── Clearing results ─────────────────────────────────────────────────────────

describe('clearFitnessResults', () => {
  const world = (index: Record<string, unknown>) => {
    const settings = new Map<string, unknown>([['model_fitness_index', index]])
    const statusCleared: string[] = []
    const transcriptsCleared: Array<string | null> = []
    for (const id of Object.keys(index)) settings.set(`model_fitness_report:${id}`, { model: id })
    return {
      settings,
      statusCleared,
      transcriptsCleared,
      deps: {
        readSetting: async <T,>(key: string, fallback: T) => (settings.has(key) ? (settings.get(key) as T) : fallback),
        writeSetting: async (key: string, value: unknown) => {
          if (value === null) settings.delete(key)
          else settings.set(key, value)
        },
        clearEvalStatus: async (m: string) => {
          statusCleared.push(m)
        },
        clearTranscripts: async (m: string | null) => {
          transcriptsCleared.push(m)
          return 7
        },
        models: async () => [],
      } as unknown as Parameters<typeof clearFitnessResults>[1],
    }
  }

  it('drops the report, the matrix entry, the resume ledger and the transcripts', async () => {
    const w = world({ 'a/model': { at: '2026-08-01' }, 'b/model': { at: '2026-08-02' } })

    const out = await clearFitnessResults('a/model', w.deps)

    expect(out).toMatchObject({ models: ['a/model'], reports: 1, transcripts: 7 })
    expect(w.settings.has('model_fitness_report:a/model')).toBe(false)
    // THE HALF EVERYONE FORGETS. Leaving the resume ledger behind makes the model
    // read as untested and then resume into a run that is already finished — a
    // Start that returns instantly having bought nothing.
    expect(w.statusCleared).toEqual(['a/model'])
    expect(w.transcriptsCleared).toEqual(['a/model'])
    // The other model is untouched.
    expect(Object.keys(w.settings.get('model_fitness_index') as object)).toEqual(['b/model'])
    expect(w.settings.has('model_fitness_report:b/model')).toBe(true)
  })

  it('clears every tested candidate when given no model', async () => {
    const w = world({ 'a/model': {}, 'b/model': {} })

    const out = await clearFitnessResults(null, w.deps)

    expect(out.models.sort()).toEqual(['a/model', 'b/model'])
    expect(out.reports).toBe(2)
    expect(Object.keys(w.settings.get('model_fitness_index') as object)).toEqual([])
    expect(w.statusCleared.sort()).toEqual(['a/model', 'b/model'])
    // One sweep of the table, not one per model.
    expect(w.transcriptsCleared).toEqual([null])
  })

  it('is not Forget: it never touches a measured capability', async () => {
    // The distinction the two dialogs exist to keep: Forget drops what a model
    // CAN DO (nine probe calls somebody paid for); this drops what a RUN FOUND.
    // `SurfaceDeps.forget` is the capability eraser and must not be reachable
    // from here at all.
    const w = world({ 'a/model': {} })
    const forgot: string[] = []
    await clearFitnessResults('a/model', { ...w.deps, forget: async (k: string) => void forgot.push(k) } as never)
    expect(forgot).toEqual([])
  })
})


// ── Speed ────────────────────────────────────────────────────────────────────

describe('speedOf', () => {
  const c = (over: Partial<EvalCaseScore>): EvalCaseScore => evalCase(over)

  it('is the median and p95 of the cases that were actually measured', () => {
    const s = speedOf(
      [1000, 2000, 3000, 4000, 90_000].map((latencyMs, i) =>
        c({ case: `c${i}`, latencyMs, startedAt: new Date(1_000_000 + i * 1000).toISOString(), wallMs: latencyMs }),
      ),
      1,
    )!
    expect(s.p50).toBe(3000)
    expect(s.p95).toBe(90_000)
    expect(s.concurrency).toBe(1)
    // A median over five is a different claim from a median over two hundred,
    // and a supplemental pass makes small samples ordinary.
    expect(s.sample).toBe(5)
  })

  it('ignores cases that never called the model — their zeros would flatter it', () => {
    // A skipped case and one the provider never answered have no latency to
    // speak of. Averaging their zeros in would make a badly-served deployment
    // read as a fast model, which is the exact inversion this page exists to
    // prevent.
    const s = speedOf(
      [
        c({ case: 'a', latencyMs: 4000, startedAt: new Date(0).toISOString(), wallMs: 4000 }),
        c({ case: 'b', latencyMs: 0, skipped: 'no tool loop on this candidate' }),
        c({ case: 'c', latencyMs: 0, skipped: 'rate limits on every attempt' }),
      ],
      1,
    )!
    expect(s.p50).toBe(4000)
  })

  it('measures elapsed as a TIMELINE, so concurrency does not inflate it', () => {
    // Four cases of 10s each, all started together, is ten seconds of wall clock
    // — not forty. A sum would report the sweep as four times longer than an
    // admin waited, and the whole reason to run wide is that it is not.
    const at = 1_700_000_000_000
    const s = speedOf(
      Array.from({ length: 4 }, (_, i) => c({ case: `c${i}`, latencyMs: 10_000, startedAt: new Date(at).toISOString(), wallMs: 10_000 })),
      4,
    )!
    expect(s.elapsedMs).toBe(10_000)
    expect(s.perMinute).toBe(24)
    expect(s.concurrency).toBe(4)
  })

  it('is null when nothing was measured, rather than zero', () => {
    // Zero would draw a Speed column reading "0ms" for a model nothing ran on,
    // which is the fastest cell on the page and a lie.
    expect(speedOf([], 1)).toBeNull()
    expect(speedOf([c({ skipped: 'never ran', latencyMs: 0 })], 1)).toBeNull()
  })
})


// ── Speed is a RATE, not a duration ──────────────────────────────────────────

describe('speedOf tokens per second', () => {
  const c = (over: Partial<EvalCaseScore>): EvalCaseScore => evalCase(over)

  it('reports the rate the model generates at', () => {
    // 300 tokens in 3s and 100 in 1s are the SAME model speed. A per-case
    // latency would call the first one three times slower, which is a fact about
    // which fixture ran rather than about the model.
    const s = speedOf(
      [
        c({ case: 'long', latencyMs: 3000, completionTokens: 300, startedAt: new Date(0).toISOString(), wallMs: 3000 }),
        c({ case: 'short', latencyMs: 1000, completionTokens: 100, startedAt: new Date(0).toISOString(), wallMs: 1000 }),
      ],
      1,
    )!
    expect(s.tokensPerSecond).toBe(100)
    // The duration is still there — "how long do I wait for a fixture" is also a
    // real question, it is just not the one a column comparing models answers.
    // (Nearest-rank over two samples takes the lower; see `percentile`.)
    expect(s.p50).toBe(1000)
    expect(s.p95).toBe(3000)
  })

  it('takes the median per case rather than total tokens over total time', () => {
    // An aggregate would be inflated by concurrency (the wall clock overlaps) and
    // dominated by one long generation. Each case's own rate is a fact about that
    // request; the median of them is a fact about the model.
    const s = speedOf(
      [
        c({ case: 'a', latencyMs: 1000, completionTokens: 10, startedAt: new Date(0).toISOString(), wallMs: 1000 }),
        c({ case: 'b', latencyMs: 1000, completionTokens: 20, startedAt: new Date(0).toISOString(), wallMs: 1000 }),
        c({ case: 'c', latencyMs: 1000, completionTokens: 3000, startedAt: new Date(0).toISOString(), wallMs: 1000 }),
      ],
      1,
    )!
    expect(s.tokensPerSecond).toBe(20)
  })

  it('is null when nothing generated enough to measure a rate', () => {
    // A sweep of contract failures produces no completions. Zero would read as
    // the slowest model on the page rather than as an absent measurement.
    const s = speedOf([c({ latencyMs: 500, completionTokens: 0, startedAt: new Date(0).toISOString(), wallMs: 500 })], 1)!
    expect(s.tokensPerSecond).toBeNull()
    expect(s.p50).toBe(500)
  })
})
