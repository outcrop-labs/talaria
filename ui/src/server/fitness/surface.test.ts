import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import {
  drilldown,
  estimateRun,
  evictArchive,
  forgetModel,
  indexEntryOf,
  keysFor,
  mergeFact,
  modelRows,
  priceOf,
  POOLED_DISAGREEMENT,
  startFitnessRun,
  usdOf,
  type FitnessIndex,
  type FitnessIndexEntry,
  type SurfaceDeps,
} from '@/server/fitness/surface'
import type { CapabilityFact } from '@/server/harness/capability'
import type { EvalCaseScore, EvalSweep } from '@/server/fitness/evals'
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
  contractHeld: true,
  firstPass: true,
  repairs: 0,
  answered: true,
  task: 'pass',
  taskError: null,
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
      at,
    })
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
      at: '2026-08-01T00:00:00.000Z',
    })
  })

  it('is unknown for a model nothing has measured', () => {
    expect(mergeFact([])).toEqual({ state: 'unknown', source: null, detail: null, score: null, at: null })
    expect(mergeFact([undefined])).toEqual({ state: 'unknown', source: null, detail: null, score: null, at: null })
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
              { id: 'json', calls: 3, promptTokens: 100, completionTokens: 20 },
              { id: 'vision', calls: 0, promptTokens: 500, completionTokens: 40 },
              { id: 'tools', calls: 0, promptTokens: 200, completionTokens: 30 },
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
  const deps = (forgotten: string[]): Partial<SurfaceDeps> => ({
    models: async () => [{ id: 'qwen3-14b', endpoints: ['spark', 'local'], qualified: false }],
    capabilities: async () => ({}),
    forget: async (key) => {
      forgotten.push(key)
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
})
