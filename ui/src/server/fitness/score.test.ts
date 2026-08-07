import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import {
  bindSlots,
  CONTRACT_READY,
  CONTRACT_UNFIT,
  declaredEdges,
  DEFAULT_TASK_FLOOR,
  fitnessSlots,
  REPAIR_WORKABLE,
  rolesReaching,
  scoreFitness,
  slotKey,
  TASK_FLOORS,
  taskFloorFor,
  type FitnessInput,
  type FitnessSlot,
  type SlotBinding,
} from '@/server/fitness/score'
import { defineHarness, type HarnessDefinition } from '@/server/harness/define'
import { builtinActivityHarnesses, type HarnessSource, type RegisteredHarness } from '@/server/harness/registry'
import type { EvalCaseScore, EvalSweep, HarnessScore } from '@/server/fitness/evals'
import type { CapabilityFact } from '@/server/harness/capability'

// The scorer is PURE by construction — every edge is an argument — so this file
// drives the real band boundaries with no gateway, no database and no model
// anywhere near it. The registry is real where the question is about the
// registry (the binding, the declared-edge table) and synthetic where the
// question is about arithmetic.

// ── Fixtures ─────────────────────────────────────────────────────────────────

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

const harness = (id: string, over: Partial<Pick<HarnessDefinition<{ q: string }, { a: string }>, 'requires' | 'model' | 'evals'>> = {}): RegisteredHarness =>
  reg(
    defineHarness<{ q: string }, { a: string }>({
      id,
      label: id,
      job: 'Answers.',
      requires: over.requires ?? [],
      floor: FLOOR,
      model: over.model ?? { chain: [] },
      render: (input) => [{ role: 'user', content: input.q }],
      output: { kind: 'json', schema: z.object({ a: z.string() }) },
      onFailure: 'null',
      evals: over.evals ?? [{ name: 'one', input: { q: 'x' }, check: () => null }],
    }),
  )

const score = (id: string, over: Partial<HarnessScore> = {}): HarnessScore => ({
  id,
  label: id,
  source: 'builtin',
  outputKind: 'json',
  tools: 'none',
  requires: [],
  verifies: true,
  repairable: true,
  cases: 10,
  skipped: 0,
  skipReason: null,
  scored: 10,
  contractRate: 1,
  repairRate: 1,
  repairYield: null,
  taskScore: 1,
  bandScores: { easy: null, standard: 1, hard: null },
  guardRate: 0,
  answeredRate: 1,
  latencyP50: 100,
  latencyP95: 100,
  promptTokens: 0,
  completionTokens: 0,
  costUsd: null,
  estimated: false,
  timeouts: 0,
  optimistic: 0,
  ...over,
})

const kase = (harnessId: string, over: Partial<EvalCaseScore> = {}): EvalCaseScore => ({
  harness: harnessId,
  case: 'one',
  band: 'standard',
  skipped: null,
  contractHeld: true,
  firstPass: true,
  repairs: 0,
  answered: true,
  task: 'pass',
  taskError: null,
  findings: 0,
  latencyMs: 100,
  promptTokens: 0,
  completionTokens: 0,
  costUsd: null,
  estimated: false,
  timedOut: false,
  optimistic: false,
  error: null,
  prompt: null,
  raw: null,
  ...over,
})

const sweep = (over: Partial<EvalSweep> = {}): EvalSweep => ({
  model: 'candidate',
  state: 'done',
  startedAt: null,
  finishedAt: null,
  done: 0,
  total: 0,
  error: null,
  harnesses: [],
  cases: [],
  unfixtured: [],
  guarded: true,
  ...over,
})

const fact = (value: boolean): CapabilityFact => ({ value, source: 'probe', at: new Date().toISOString(), score: value ? 1 : 0 })

/** One synthetic slot with one harness bound, so a band boundary can be driven
 *  without dragging twenty real slots through every assertion. */
const oneSlot = (h: RegisteredHarness, over: Partial<FitnessSlot> = {}): SlotBinding[] => [
  {
    slot: { kind: 'role', id: 'utility', label: 'Utility', hint: '', requires: [], live: true, ...over },
    harnesses: [{ id: h.id, via: 'chain' }],
  },
]

const run = (input: Partial<FitnessInput> & Pick<FitnessInput, 'sweep' | 'harnesses'>, bindings: SlotBinding[]) =>
  scoreFitness({ capabilities: {}, ...input }, bindings)

// ── Slots and binding ────────────────────────────────────────────────────────

describe('slots', () => {
  it('covers both assignment registries and nothing else', () => {
    const slots = fitnessSlots()
    expect(slots.filter((s) => s.kind === 'role')).toHaveLength(11)
    expect(slots.filter((s) => s.kind === 'agent')).toHaveLength(9)
    // The reserved roles and the non-assignable briefer are still slots — they
    // just say they are inert.
    expect(slots.find((s) => s.id === 'vision')?.live).toBe(false)
    expect(slots.find((s) => s.kind === 'agent' && s.id === 'briefer')?.live).toBe(false)
  })

  it('keys a role and an agent apart even if the ids ever collide', () => {
    expect(slotKey({ kind: 'role', id: 'utility' })).toBe('role:utility')
    expect(slotKey({ kind: 'agent', id: 'utility' })).toBe('agent:utility')
  })
})

describe('rolesReaching', () => {
  it('finds the Utility role through the DEFAULT chain, which is what ten builtins use', async () => {
    // The titler declares only a pin. Nothing in this file spells the default
    // chain — the real resolver is asked, which is the point.
    expect(await rolesReaching({ pin: 'titler' })).toContain('utility')
  })

  it('finds a role a spec names explicitly', async () => {
    expect(await rolesReaching({ role: 'code-heavy' })).toEqual(expect.arrayContaining(['code-heavy', 'utility']))
  })

  it('finds nothing for an empty chain — the model comes from the subject of the call', async () => {
    expect(await rolesReaching({ chain: [] })).toEqual([])
  })

  it('does not invent a role binding from a chain that has no role step', async () => {
    expect(await rolesReaching({ chain: ['env', 'first-routable'] })).toEqual([])
  })
})

describe('bindSlots', () => {
  it('binds the pinned utility harnesses to role:utility and to their own agent slot', async () => {
    const bindings = await bindSlots(builtinActivityHarnesses())
    const utility = bindings.find((b) => slotKey(b.slot) === 'role:utility')
    expect(utility?.harnesses.map((h) => h.id)).toEqual(expect.arrayContaining(['titler', 'summarizer', 'librarian', 'blurb-writer']))
    const titler = bindings.find((b) => slotKey(b.slot) === 'agent:titler')
    expect(titler?.harnesses).toEqual([{ id: 'titler', via: 'pin' }])
  })

  it('binds the judge through registry.platformAgentOf, whose model lives in judge_config rather than in a pin', async () => {
    const bindings = await bindSlots(builtinActivityHarnesses())
    expect(bindings.find((b) => slotKey(b.slot) === 'agent:judge')?.harnesses).toEqual([{ id: 'judge', via: 'pin' }])
    // And NOT to role:utility — its chain is ['env', 'first-routable'].
    expect(bindings.find((b) => slotKey(b.slot) === 'role:utility')?.harnesses.map((h) => h.id)).not.toContain('judge')
  })

  it('carries the one declared edge, so the research columns are not empty', async () => {
    const bindings = await bindSlots(builtinActivityHarnesses())
    for (const role of ['research-recon', 'research-brief', 'research-expedition'] as const) {
      expect(bindings.find((b) => slotKey(b.slot) === `role:${role}`)?.harnesses).toEqual([{ id: 'research-search', via: 'declared' }])
    }
  })

  it('locks every declared edge against the real registry — a typo cannot invent a binding', () => {
    const ids = new Set(builtinActivityHarnesses().map((h) => h.id))
    for (const edge of declaredEdges()) expect(ids.has(edge.harness), `${edge.harness} is not a registered harness`).toBe(true)
  })

  it('keeps a slot nothing reaches, with an empty list rather than dropping the column', async () => {
    const bindings = await bindSlots(builtinActivityHarnesses())
    expect(bindings.find((b) => slotKey(b.slot) === 'role:embedding')?.harnesses).toEqual([])
  })
})

// ── The three bands ──────────────────────────────────────────────────────────

describe('bands', () => {
  it('is ready when every requirement is measured true, contract is at the ceiling and task is at the floor', () => {
    const h = harness('h', { requires: ['json'] })
    const report = run(
      {
        sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }),
        harnesses: [h],
        capabilities: { json: fact(true) },
      },
      oneSlot(h),
    )
    expect(report.slots[0]?.band).toBe('ready')
    expect(report.slots[0]?.reasons).toEqual([])
  })

  it('is workable exactly at the ready contract boundary minus a hair, and ready at it', () => {
    const h = harness('h')
    const rate = CONTRACT_READY - 0.01
    const at = run({ sweep: sweep({ harnesses: [score('h', { contractRate: CONTRACT_READY })], cases: [kase('h')] }), harnesses: [h] }, oneSlot(h))
    // repairRate pinned to contractRate so the weakness being named is the
    // contract itself and not the repair turn.
    const below = run({ sweep: sweep({ harnesses: [score('h', { contractRate: rate, repairRate: rate })], cases: [kase('h')] }), harnesses: [h] }, oneSlot(h))
    expect(at.slots[0]?.band).toBe('ready')
    expect(below.slots[0]?.band).toBe('workable')
    expect(below.slots[0]?.reasons[0]?.kind).toBe('contract')
  })

  it('names the repair turn, not the contract, when a sub-ceiling first pass is fully rescued', () => {
    const h = harness('h')
    const report = run(
      { sweep: sweep({ harnesses: [score('h', { contractRate: CONTRACT_READY - 0.01, repairRate: 1 })], cases: [kase('h')] }), harnesses: [h] },
      oneSlot(h),
    )
    expect(report.slots[0]?.reasons[0]?.kind).toBe('repair-carried')
  })

  it('is unfit below the contract floor and names the harness, never a bare score', () => {
    const h = harness('h')
    const report = run(
      {
        sweep: sweep({
          harnesses: [score('h', { contractRate: CONTRACT_UNFIT - 0.01, repairRate: 0.5 })],
          cases: [kase('h', { contractHeld: false, firstPass: false, task: 'unscored', error: 'the reply never closed its JSON value' })],
        }),
        harnesses: [h],
      },
      oneSlot(h),
    )
    expect(report.slots[0]?.band).toBe('unfit')
    const reason = report.slots[0]?.reasons[0]
    expect(reason?.kind).toBe('contract')
    expect(reason?.harness).toBe('h')
    expect(reason?.assertion).toBe('the reply never closed its JSON value')
  })

  it('40/95 is workable and says the repair path is what carries it; 40/45 is not', () => {
    const h = harness('h')
    const carried = run(
      { sweep: sweep({ harnesses: [score('h', { contractRate: 0.4, repairRate: REPAIR_WORKABLE })], cases: [kase('h')] }), harnesses: [h] },
      oneSlot(h),
    )
    expect(carried.slots[0]?.band).toBe('workable')
    expect(carried.slots[0]?.reasons.map((r) => r.kind)).toContain('repair-carried')

    const stranded = run({ sweep: sweep({ harnesses: [score('h', { contractRate: 0.4, repairRate: 0.45 })], cases: [kase('h')] }), harnesses: [h] }, oneSlot(h))
    expect(stranded.slots[0]?.band).toBe('unfit')
  })

  it('never claims the repair path carried a TEXT harness, where no repair turn is ever sent', () => {
    const h = harness('h')
    // repairable: false mirrors run.ts setting maxRepairs to 0 for text output.
    const report = run(
      {
        sweep: sweep({ harnesses: [score('h', { repairable: false, outputKind: 'text', contractRate: 0.4, repairRate: 0.4 })], cases: [kase('h')] }),
        harnesses: [h],
      },
      oneSlot(h),
    )
    expect(report.slots[0]?.band).toBe('unfit')
    expect(report.slots[0]?.reasons.map((r) => r.kind)).not.toContain('repair-carried')
  })

  it('is workable within 10% of the task floor and unfit beyond it, carrying the fixture assertion verbatim', () => {
    const h = harness('h')
    // A slot with no floor policy, so the boundary being driven is the default.
    const slot = oneSlot(h, { id: 'code-standard', label: 'Workbench · Standard effort' })
    const floor = DEFAULT_TASK_FLOOR
    const near = run(
      {
        sweep: sweep({
          harnesses: [score('h', { taskScore: floor - 0.05 })],
          cases: [kase('h', { task: 'fail', taskError: 'the title must be 3-7 words' })],
        }),
        harnesses: [h],
      },
      slot,
    )
    expect(near.slots[0]?.band).toBe('workable')

    const far = run(
      {
        sweep: sweep({
          harnesses: [score('h', { taskScore: floor * (1 - 0.1) - 0.01 })],
          cases: [kase('h', { task: 'fail', taskError: 'the title must be 3-7 words' })],
        }),
        harnesses: [h],
      },
      slot,
    )
    expect(far.slots[0]?.band).toBe('unfit')
    expect(far.slots[0]?.reasons[0]?.assertion).toBe('the title must be 3-7 words')
  })

  it('is unfit on a capability recorded false — the assignment audit 1.6 is about', () => {
    const h = harness('h', { requires: ['search'] })
    const report = run({ sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }), harnesses: [h], capabilities: { search: fact(false) } }, oneSlot(h))
    expect(report.slots[0]?.band).toBe('unfit')
    expect(report.slots[0]?.reasons[0]?.kind).toBe('missing-capability')
  })

  it('is unfit when a ROLE requires a capability the model is recorded as lacking, even with every harness green', () => {
    const h = harness('h')
    const report = run(
      { sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }), harnesses: [h], capabilities: { search: fact(false) } },
      oneSlot(h, { id: 'research-recon', label: 'Research · Recon', requires: ['search'] }),
    )
    expect(report.slots[0]?.band).toBe('unfit')
    expect(report.slots[0]?.reasons[0]?.harness).toBe(null)
  })

  it('caps at workable when a required capability was never measured — unknown is never unfit here', () => {
    const h = harness('h', { requires: ['json'] })
    const report = run({ sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }), harnesses: [h], capabilities: {} }, oneSlot(h))
    expect(report.slots[0]?.band).toBe('workable')
    expect(report.slots[0]?.reasons.map((r) => r.kind)).toContain('unmeasured-capability')
  })

  it('is unfit on a safety regression above the production baseline, and clean at or below it', () => {
    const h = harness('h')
    const withFindings = sweep({ harnesses: [score('h', { guardRate: 0.2 })], cases: [kase('h', { findings: 2 })] })
    const regressed = run({ sweep: withFindings, harnesses: [h], guardBaseline: { h: 0.1 } }, oneSlot(h))
    expect(regressed.slots[0]?.band).toBe('unfit')
    expect(regressed.slots[0]?.reasons[0]?.kind).toBe('safety')

    const tolerated = run({ sweep: withFindings, harnesses: [h], guardBaseline: { h: 0.2 } }, oneSlot(h))
    expect(tolerated.slots[0]?.band).toBe('ready')
  })

  it('treats a missing baseline as zero and SAYS SO rather than pretending it measured zero', () => {
    const h = harness('h')
    const report = run(
      { sweep: sweep({ harnesses: [score('h', { guardRate: 0.34 })], cases: [kase('h', { findings: 1 })] }), harnesses: [h] },
      oneSlot(h),
    )
    expect(report.slots[0]?.band).toBe('unfit')
    expect(report.slots[0]?.reasons[0]?.detail).toContain('nothing filed for this harness yet')
  })

  it('does not read a guard rate of zero as clean when the guard was off', () => {
    const h = harness('h')
    const report = run({ sweep: sweep({ guarded: false, harnesses: [score('h')], cases: [kase('h')] }), harnesses: [h] }, oneSlot(h))
    expect(report.slots[0]?.band).toBe('workable')
    expect(report.slots[0]?.reasons.map((r) => r.kind)).toContain('guard-off')
    expect(report.guarded).toBe(false)
  })

  it('does not blame the model when nothing answered — a refused floor or a dead gateway is untested', () => {
    const h = harness('h')
    const report = run(
      {
        sweep: sweep({
          harnesses: [score('h', { answeredRate: 0, contractRate: 0, repairRate: 0, taskScore: null })],
          cases: [kase('h', { answered: false, contractHeld: false, firstPass: false, task: 'unscored', error: 'refused below the capability floor: search' })],
        }),
        harnesses: [h],
      },
      oneSlot(h),
    )
    expect(report.slots[0]?.band).toBe('untested')
    expect(report.slots[0]?.reasons[0]?.assertion).toBe('refused below the capability floor: search')
  })

  it('is NOT unfit when a registered tool reaches the capability the model lacks', () => {
    // THE CORRECTION THIS WHOLE PASS IS ABOUT. deepseek-v4-flash: `search`
    // measured false, `tools` measured true, and a web-search server in the
    // registry. It was reported Not-a-fit for all three Research slots — a true
    // statement about the weights and a false one about the slot, which is a
    // model running inside Talaria with the tools this org registered.
    const h = harness('h', { requires: ['search'] })
    const report = run(
      {
        sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }),
        harnesses: [h],
        capabilities: { search: fact(false) },
        reach: { search: { capability: 'search', reached: true, via: 'tool', supplier: { server: 'exa', tool: 'web_search' }, detail: 'x' } },
      },
      oneSlot(h, { id: 'research-recon', label: 'Research · Recon', requires: ['search'] }),
    )
    expect(report.slots[0]?.band).toBe('ready')
    // Reported, not silent: if that server is removed the cell changes, and this
    // is the sentence that explains why.
    const supplied = report.slots[0]?.reasons.filter((r) => r.kind === 'supplied-capability') ?? []
    expect(supplied.length).toBeGreaterThan(0)
    expect(supplied[0]?.detail).toContain('exa.web_search')
  })

  it('is still unfit when nothing reaches the capability', () => {
    // Reach widens the question; it does not soften it. An org with no search
    // server and a memory-only model gets the same verdict it always got.
    const h = harness('h', { requires: ['search'] })
    const report = run(
      {
        sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }),
        harnesses: [h],
        capabilities: { search: fact(false) },
        reach: { search: { capability: 'search', reached: false, via: null, supplier: null, detail: 'no enabled MCP server offers a tool for it' } },
      },
      oneSlot(h, { id: 'research-recon', label: 'Research · Recon', requires: ['search'] }),
    )
    expect(report.slots[0]?.band).toBe('unfit')
    // And the sentence names the org's next move rather than blaming the model.
    expect(report.slots[0]?.reasons[0]?.detail).toContain('no enabled MCP server')
  })

  it('falls back to the raw capability fact when nothing asked about reach', () => {
    // Every caller that has no registry to ask gets the pre-reach verdict, which
    // is narrower and never wrong in the unsafe direction.
    const h = harness('h', { requires: ['search'] })
    const report = run({ sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }), harnesses: [h], capabilities: { search: fact(false) } }, oneSlot(h))
    expect(report.slots[0]?.band).toBe('unfit')
  })

  it('reports a harness the sweep could not run as not-runnable, not as a contract failure', () => {
    // The tool-loop harnesses on a gateway candidate. `cases: 0` with a
    // `skipReason` is how the sweep says "nothing ran"; a band of `unfit` here
    // would blame a model that was never called.
    const h = harness('h')
    const why = 'X runs the model\u2019s own tool loop, and "gw/model" is served by the org gateway, which has no tool loop.'
    const report = run(
      { sweep: sweep({ harnesses: [score('h', { cases: 0, skipped: 2, skipReason: why, contractRate: 0, repairRate: 0, taskScore: null, answeredRate: 0 })], cases: [] }), harnesses: [h] },
      oneSlot(h),
    )
    expect(report.slots[0]?.band).toBe('untested')
    expect(report.slots[0]?.reasons[0]?.kind).toBe('not-runnable')
    // The sweep's own sentence, verbatim — not "this sweep did not run it",
    // which reads as "press Test again" and would be wrong.
    expect(report.slots[0]?.reasons[0]?.detail).toBe(why)
  })

  it('states a missing capability once, not once per harness that also needs it', () => {
    // THE NOISE THIS KILLS. A research slot declares `requires: ['search']` and
    // binds a harness that requires it too, so the same fact was reported twice
    // — and the harness telling dragged the runner's whole refusal paragraph
    // with it, once per slot, burying every other reason on the page.
    const h = harness('h', { requires: ['search'] })
    const report = run(
      {
        sweep: sweep({
          harnesses: [score('h', { answeredRate: 0, contractRate: 0, repairRate: 0, taskScore: null })],
          cases: [kase('h', { answered: false, contractHeld: false, firstPass: false, task: 'unscored', error: 'a very long refusal paragraph' })],
        }),
        harnesses: [h],
        capabilities: { search: fact(false) },
      },
      oneSlot(h, { id: 'research-recon', label: 'Research \u00b7 Recon', requires: ['search'] }),
    )
    expect(report.slots[0]?.band).toBe('unfit')
    // One line, about the slot the admin is choosing for.
    expect(report.slots[0]?.reasons).toHaveLength(1)
    expect(report.slots[0]?.reasons[0]?.harness).toBe(null)
    expect(report.slots[0]?.reasons[0]?.capability).toBe('search')
    // And no `no-answer` restating the same refusal a third time.
    expect(report.slots[0]?.reasons.map((r) => r.kind)).not.toContain('no-answer')
  })

  it('keeps a harness-level capability line the slot does not itself declare', () => {
    // The dedupe is per capability, not per reason kind: a harness needing
    // something its slot never asked for is news, and must survive.
    const h = harness('h', { requires: ['vision'] })
    const report = run(
      { sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }), harnesses: [h], capabilities: { search: fact(false), vision: fact(false) } },
      oneSlot(h, { requires: ['search'] }),
    )
    expect(report.slots[0]?.reasons.map((r) => r.capability)).toEqual(['search', 'vision'])
  })
})

// ── The empty column ─────────────────────────────────────────────────────────

describe('a slot with no harness bound', () => {
  it('reads as unbound, not as an empty pass', () => {
    const report = run({ sweep: sweep(), harnesses: [] }, [
      { slot: { kind: 'role', id: 'embedding', label: 'Embeddings', hint: '', requires: [], live: false }, harnesses: [] },
    ])
    expect(report.slots[0]?.band).toBe('unbound')
    expect(report.slots[0]?.reasons[0]?.kind).toBe('no-harness')
    expect(report.slots[0]?.reasons[0]?.detail).toContain('This is not a pass')
    expect(report.slots[0]?.contract).toBe(null)
  })

  it('still reports a capability the role is recorded as lacking, so an unbound column is not a hiding place', () => {
    const report = run({ sweep: sweep(), harnesses: [], capabilities: { search: fact(false) } }, [
      { slot: { kind: 'role', id: 'research-recon', label: 'Research · Recon', hint: '', requires: ['search'], live: true }, harnesses: [] },
    ])
    expect(report.slots[0]?.band).toBe('unfit')
  })

  it('cannot be Ready when one of several bound harnesses has no verdict', () => {
    const good = harness('good')
    const silent = harness('silent')
    const report = run({ sweep: sweep({ harnesses: [score('good')], cases: [kase('good')] }), harnesses: [good, silent] }, [
      {
        slot: { kind: 'role', id: 'utility', label: 'Utility', hint: '', requires: [], live: true },
        harnesses: [
          { id: 'good', via: 'chain' },
          { id: 'silent', via: 'chain' },
        ],
      },
    ])
    expect(report.slots[0]?.band).toBe('workable')
    expect(report.slots[0]?.reasons.map((r) => r.kind)).toContain('partial-coverage')
  })

  it('says a harness declares no fixtures rather than scoring it', () => {
    const bare = harness('bare', { evals: [] })
    const report = run({ sweep: sweep(), harnesses: [bare] }, oneSlot(bare))
    expect(report.slots[0]?.band).toBe('untested')
    expect(report.slots[0]?.reasons[0]?.kind).toBe('no-fixtures')
  })
})

// ── Aggregates ───────────────────────────────────────────────────────────────

describe('cross-harness aggregates', () => {
  it('weights by CASE and carries a label saying so — schema_valid is comparable per harness only', () => {
    const a = harness('a')
    const b = harness('b')
    const report = run(
      {
        sweep: sweep({
          harnesses: [score('a', { cases: 1, contractRate: 0, repairRate: 0, taskScore: null }), score('b', { cases: 3, contractRate: 1 })],
          cases: [
            kase('a', { case: 'a1', firstPass: false, contractHeld: false, task: 'unscored' }),
            kase('b', { case: 'b1' }),
            kase('b', { case: 'b2' }),
            kase('b', { case: 'b3' }),
          ],
        }),
        harnesses: [a, b],
      },
      [
        {
          slot: { kind: 'role', id: 'utility', label: 'Utility', hint: '', requires: [], live: true },
          harnesses: [
            { id: 'a', via: 'chain' },
            { id: 'b', via: 'chain' },
          ],
        },
      ],
    )
    // 3 of 4 cases, not the mean of 0 and 1.
    expect(report.slots[0]?.contract?.rate).toBeCloseTo(0.75)
    expect(report.slots[0]?.contract?.label).toContain('comparable within a harness only')
    // The band still comes from the WORST harness, not from the aggregate.
    expect(report.slots[0]?.band).toBe('unfit')
  })

  it('scores the fixture check only over cases that were gradable', () => {
    const h = harness('h')
    const report = run(
      {
        sweep: sweep({
          harnesses: [score('h', { cases: 2, taskScore: 0.5 })],
          cases: [kase('h', { case: '1' }), kase('h', { case: '2', task: 'unscored' })],
        }),
        harnesses: [h],
      },
      oneSlot(h),
    )
    expect(report.slots[0]?.task).toEqual(expect.objectContaining({ numerator: 1, denominator: 1 }))
  })
})

// ── Floors ───────────────────────────────────────────────────────────────────

describe('task floors', () => {
  it('is per slot, so one harness can be ready for Utility and workable for a stricter slot', () => {
    const h = harness('h')
    const s = sweep({ harnesses: [score('h', { taskScore: 0.75 })], cases: [kase('h')] })
    const utility = run({ sweep: s, harnesses: [h] }, oneSlot(h))
    const judge = run({ sweep: s, harnesses: [h] }, oneSlot(h, { kind: 'agent', id: 'judge', label: 'Judge' }))
    expect(utility.slots[0]?.band).toBe('ready')
    expect(judge.slots[0]?.band).toBe('unfit')
    expect(utility.slots[0]?.taskFloor).toBe(TASK_FLOORS['role:utility'])
    expect(judge.slots[0]?.taskFloor).toBe(TASK_FLOORS['agent:judge'])
  })

  it('takes an install override ahead of the shipped policy', () => {
    const slot: FitnessSlot = { kind: 'role', id: 'utility', label: 'Utility', hint: '', requires: [], live: true }
    expect(taskFloorFor(slot)).toBe(TASK_FLOORS['role:utility'])
    expect(taskFloorFor(slot, { 'role:utility': 0.99 })).toBe(0.99)
  })

  it('falls back to the default for a slot with no policy', () => {
    expect(taskFloorFor({ kind: 'role', id: 'code-standard', label: '', hint: '', requires: [], live: true })).toBe(DEFAULT_TASK_FLOOR)
  })
})

// ── Unbound harnesses ────────────────────────────────────────────────────────

describe('unbound harnesses', () => {
  it('are scored on their own, because "can this model work a ticket" still has an answer', () => {
    const h = harness('work-session')
    const report = run({ sweep: sweep({ harnesses: [score('work-session')], cases: [kase('work-session')] }), harnesses: [h] }, [])
    expect(report.unbound.map((v) => v.harness)).toEqual(['work-session'])
    expect(report.unbound[0]?.band).toBe('ready')
  })

  it('excludes anything a slot already claims', () => {
    const h = harness('h')
    const report = run({ sweep: sweep({ harnesses: [score('h')], cases: [kase('h')] }), harnesses: [h] }, oneSlot(h))
    expect(report.unbound).toEqual([])
  })
})
