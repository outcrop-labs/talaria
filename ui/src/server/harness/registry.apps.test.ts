// THE APP-SHIPPED LAYER of the activity registry, and the payoff that justifies
// it: a harness an APP ships, with fixtures, turns up in the org's model-fitness
// matrix without the platform knowing anything about that app.
//
// Everything here is driven through the REAL surfaces — the harness is built
// with the `defineHarness` an app author imports from '@talaria/sdk/server', the
// registry merge is `listActivityHarnesses`, and the sweep is `runEvalSweep`
// over `runHarness`. Only the two edges an app layer cannot have in a test are
// injected: the build-time module glob, and the settings row that says which
// apps an admin switched on.
import { describe, expect, it, vi } from 'vitest'
import { z } from 'zod'
import { defineHarness, defineWorkbenchHarness, type WorkbenchHarnessDefinition } from '@/sdk/server'
import { builtinActivityHarnesses, listActivityHarnesses, type AppHarnessLayer, type RegisteredHarness } from './registry'
import { runEvalSweep, IDLE_STATUS, type EvalSweepStatus } from '@/server/fitness/evals'
import type { HarnessDeps, TransportReply } from './run'
import type { GuardConfig } from '@/server/guardrails'

// ── A fixture app ────────────────────────────────────────────────────────────

const TRIAGE = z.object({ priority: z.string(), why: z.string() })

/** What an app author writes: a definition, a floor, and two fixtures. Nothing
 *  in it names Talaria's internals — the whole surface is the SDK import above. */
const triageHarness = defineHarness<{ subject: string }, z.infer<typeof TRIAGE>>({
  id: 'notes:triage',
  label: 'Notes — triage',
  job: 'Sorts an incoming note into a priority.',
  requires: ['json'],
  floor: { capabilities: [], refuseBelow: false, note: 'Runs on anything that can return an object.' },
  model: { chain: [] },
  render: (input) => [{ role: 'user', content: `triage: ${input.subject}` }],
  output: {
    kind: 'json',
    schema: TRIAGE,
    verify: (value, input) => (value.why.includes(input.subject) ? null : `the reason must mention '${input.subject}'`),
  },
  onFailure: 'null',
  guard: { rules: ['ungrounded_ref'] },
  evals: [
    { name: 'urgent note', input: { subject: 'outage' }, check: (v) => (v.priority === 'high' ? null : `expected a high priority, got '${v.priority}'`) },
    { name: 'quiet note', input: { subject: 'typo' }, check: (v) => (v.priority === 'low' ? null : `expected a low priority, got '${v.priority}'`) },
  ],
})

const modules = (mods: Record<string, () => Promise<unknown>>): AppHarnessLayer['modules'] => mods

/** The layer with a fixture app installed and enabled. */
function layerOf(mods: Record<string, () => Promise<unknown>>, enabled: string[], warn = vi.fn()): Partial<AppHarnessLayer> {
  return { modules: modules(mods), enabled: async () => new Set(enabled), warn }
}

const NOTES = '../../../../apps/notes/harnesses/triage.ts'

const BUILTIN_IDS = builtinActivityHarnesses().map((h) => h.id)

// ── Discovery ────────────────────────────────────────────────────────────────

describe('the app-shipped layer', () => {
  it('enumerates a harness an app ships, with its fixtures', async () => {
    const merged = await listActivityHarnesses(layerOf({ [NOTES]: async () => ({ default: triageHarness }) }, ['notes']))
    const mine = merged.find((h) => h.id === 'notes:triage')
    expect(mine?.source).toBe('app:notes')
    expect(mine?.evalNames).toEqual(['urgent note', 'quiet note'])
    // The metadata the admin panel reads comes off the app's definition, not
    // off a platform default.
    expect(mine?.label).toBe('Notes — triage')
    expect(mine?.outputKind).toBe('json')
    // ...and it hands the definition back with I and O still paired, which is
    // what lets the sweep run it against its own fixtures.
    expect(mine?.use((def) => def.evals?.length)).toBe(2)
  })

  it('follows the app enablement switch, exactly as the workbench registry does', async () => {
    // A disabled app has no nav presence and its server routes 404. A harness it
    // ships appearing in Admin -> Models would be that same app, on.
    const merged = await listActivityHarnesses(layerOf({ [NOTES]: async () => ({ default: triageHarness }) }, []))
    expect(merged.map((h) => h.id)).toEqual(BUILTIN_IDS)
  })

  it('merges builtin < app-shipped < admin-custom, by id', async () => {
    // An app that ships a harness with a builtin's id REPLACES it — nothing
    // merges field by field, because a half-overridden prompt with somebody
    // else's schema under it is not a harness anybody can reason about.
    const override = { ...triageHarness, id: 'titler', label: "Notes' own titler" }
    const merged = await listActivityHarnesses(layerOf({ [NOTES]: async () => ({ default: override }) }, ['notes']))

    expect(merged.map((h) => h.id)).toEqual(BUILTIN_IDS)
    const titler = merged.find((h) => h.id === 'titler')
    expect(titler?.source).toBe('app:notes')
    expect(titler?.label).toBe("Notes' own titler")
    // Replaced in place: the admin panel's reading order is stable whether or
    // not an app overrode something in it.
    expect(merged.findIndex((h) => h.id === 'titler')).toBe(BUILTIN_IDS.indexOf('titler'))
  })

  it('resolves two apps claiming one id the same way on every process', async () => {
    // Arbitrary but STABLE: paths are walked sorted, later wins. A merge that
    // depended on filesystem or glob order would give two nodes of one
    // deployment different registries.
    const alpha = { ...triageHarness, label: 'from alpha' }
    const zeta = { ...triageHarness, label: 'from zeta' }
    const mods = {
      '../../../../apps/zeta/harnesses/triage.ts': async () => ({ default: zeta }),
      '../../../../apps/alpha/harnesses/triage.ts': async () => ({ default: alpha }),
    }
    const merged = await listActivityHarnesses(layerOf(mods, ['alpha', 'zeta']))
    const mine = merged.find((h) => h.id === 'notes:triage')
    expect(mine?.label).toBe('from zeta')
    expect(mine?.source).toBe('app:zeta')
  })
})

// ── Safety ───────────────────────────────────────────────────────────────────

describe('a broken app', () => {
  /** Every way an app module can be wrong, and each one must cost exactly that
   *  app. The list is the document: adding a required field to the definition
   *  means adding the way it can be missing here. */
  const BAD: Array<[string, unknown]> = [
    ['no default export at all', undefined],
    ['a default that is not an object', 'harness'],
    ['no id', { ...triageHarness, id: '' }],
    ['no label', { ...triageHarness, label: '   ' }],
    ['no job for the admin panel', { ...triageHarness, job: undefined }],
    ['no render', { ...triageHarness, render: undefined }],
    ['no floor', { ...triageHarness, floor: undefined }],
    ['a floor with no refusal decision', { ...triageHarness, floor: { capabilities: [], note: 'x' } }],
    ['no model spec', { ...triageHarness, model: undefined }],
    ['an output kind nothing implements', { ...triageHarness, output: { kind: 'yaml' } }],
    ['a json output with no schema to parse with', { ...triageHarness, output: { kind: 'json', schema: {} } }],
    ['no failure policy', { ...triageHarness, onFailure: 'shrug' }],
    ['fixtures that are not an array', { ...triageHarness, evals: { name: 'x' } }],
    ['a fixture with no check', { ...triageHarness, evals: [{ name: 'x', input: {} }] }],
    ['a fixture with no name', { ...triageHarness, evals: [{ name: '', input: {}, check: () => null }] }],
  ]

  it.each(BAD)('is skipped, not fatal: %s', async (_why, value) => {
    const warn = vi.fn()
    const merged = await listActivityHarnesses(layerOf({ [NOTES]: async () => ({ default: value }) }, ['notes'], warn))
    // THE ASSERTION THAT MATTERS: the platform's own 23 are all still there.
    // This list is read by the fitness matrix and the admin panel, and one
    // third-party app must not be able to empty either page.
    expect(merged.map((h) => h.id)).toEqual(BUILTIN_IDS)
    expect(warn).toHaveBeenCalledTimes(1)
    expect(warn.mock.calls[0]?.[0]).toContain('notes')
  })

  it('cannot displace a builtin with a definition that was rejected', async () => {
    // The sharp version of the case above: an app ships something malformed
    // under a builtin's id. The rejected value must never reach the map — a
    // registry that deleted the titler because an app half-wrote one would take
    // a core feature down with a typo in a third-party file.
    const merged = await listActivityHarnesses(layerOf({ [NOTES]: async () => ({ default: { ...triageHarness, id: 'titler', render: undefined } }) }, ['notes']))
    expect(merged.find((h) => h.id === 'titler')?.source).toBe('builtin')
  })

  it('logs and skips an app that throws on import', async () => {
    const warn = vi.fn()
    const mods = {
      [NOTES]: async () => {
        throw new Error('Cannot find module @notes/parser')
      },
      '../../../../apps/other/harnesses/triage.ts': async () => ({ default: triageHarness }),
    }
    const merged = await listActivityHarnesses(layerOf(mods, ['notes', 'other'], warn))
    expect(warn.mock.calls[0]?.[0]).toContain('Cannot find module')
    // ...and the healthy app on the other side of the broken one still loads.
    // A loop that gave up on the first throw would make one bad app a silent
    // outage for every app installed after it alphabetically.
    expect(merged.find((h) => h.id === 'notes:triage')?.source).toBe('app:other')
  })

  it("keeps the platform's harnesses when the layer itself fails", async () => {
    const merged = await listActivityHarnesses({
      modules: modules({ [NOTES]: async () => ({ default: triageHarness }) }),
      enabled: async () => {
        throw new Error('settings unavailable')
      },
    })
    expect(merged.map((h) => h.id)).toEqual(BUILTIN_IDS)
  })

  it('asks nothing about enablement when no app ships a harness', async () => {
    // The common install, and the reason this is asserted: `enabled` is a
    // settings read, and `listActivityHarnesses` is called by every fitness
    // sweep and every admin page load.
    const enabled = vi.fn(async () => new Set<string>())
    const merged = await listActivityHarnesses({ modules: {}, enabled })
    expect(enabled).not.toHaveBeenCalled()
    expect(merged.map((h) => h.id)).toEqual(BUILTIN_IDS)
  })
})

// ── The payoff ───────────────────────────────────────────────────────────────

const CONFIG: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

/** The sweep's edges: a transport that answers from a list, and a status row in
 *  memory. Model resolution never runs — the sweep pins the candidate. */
function sweepDeps(harnesses: () => Promise<RegisteredHarness[]>, replies: string[]) {
  let n = 0
  const state = { status: { ...IDLE_STATUS } as EvalSweepStatus }
  const harnessDeps: Partial<HarnessDeps> = {
    transport: async (): Promise<TransportReply> => {
      const text = replies[Math.min(n++, replies.length - 1)] ?? ''
      return { kind: 'gateway', text, toolNames: [], usage: { promptTokens: 20, completionTokens: 8 }, contractDropped: false }
    },
    routing: async (model) => ({ endpoints: ['spark'], upstreamModel: model }),
    personaKeys: async () => [],
    missingCapabilities: async () => [],
    capabilities: async () => ({}),
    guardConfig: async () => CONFIG,
    guardText: async () => [],
    now: () => Date.now(),
  }
  return {
    harnesses,
    harnessDeps,
    readStatus: async () => state.status,
    writeStatus: async (s: EvalSweepStatus) => {
      state.status = JSON.parse(JSON.stringify(s)) as EvalSweepStatus
    },
    price: async () => null,
    now: () => 1_700_000_000_000,
  }
}

describe('an app-shipped harness in the fitness matrix', () => {
  it('is swept and scored without the platform knowing the app exists', async () => {
    // THE WHOLE ARGUMENT for putting fixtures in the registry rather than in a
    // test directory: an app tells an admin which of their models it works on,
    // and it costs the app author an array. Nothing below names the app — the
    // sweep iterates the registry, the registry found the app.
    const harnesses = () => listActivityHarnesses(layerOf({ [NOTES]: async () => ({ default: triageHarness }) }, ['notes']))
    const sweep = await runEvalSweep('qwen3-14b', {
      only: ['notes:triage'],
      restart: true,
      deps: sweepDeps(harnesses, [JSON.stringify({ priority: 'high', why: 'the outage is live' }), JSON.stringify({ priority: 'low', why: 'a typo can wait' })]),
    })

    expect(sweep.state).toBe('done')
    expect(sweep.total).toBe(2)
    const column = sweep.harnesses.find((h) => h.id === 'notes:triage')
    expect(column?.source).toBe('app:notes')
    expect(column?.cases).toBe(2)
    // The contract held on the first attempt of both, and both fixtures' own
    // checks passed — the two numbers the matrix cell is made of.
    expect(column?.contractRate).toBe(1)
    expect(column?.taskScore).toBe(1)
    // The app declared `verify`, so its column can report the input-relational
    // half of its contract rather than only the schema's half.
    expect(column?.verifies).toBe(true)
  })

  it("reports the app's own fixture sentence when the model gets it wrong", async () => {
    // The drill-down an admin reads is written by the APP, verbatim. That is
    // what makes a red cell in someone else's column actionable.
    const harnesses = () => listActivityHarnesses(layerOf({ [NOTES]: async () => ({ default: triageHarness }) }, ['notes']))
    const sweep = await runEvalSweep('tiny-1b', {
      only: ['notes:triage'],
      restart: true,
      deps: sweepDeps(harnesses, [JSON.stringify({ priority: 'low', why: 'the outage seems fine' })]),
    })

    const first = sweep.cases[0]
    expect(first?.contractHeld).toBe(true)
    expect(first?.task).toBe('fail')
    expect(first?.taskError).toBe("expected a high priority, got 'low'")
    // The contract held and the fixture rejected the value: `optimistic`, which
    // is exactly the reading the app author wants surfaced rather than buried.
    expect(first?.optimistic).toBe(true)
    // ...and the drill-down carries the actual prompt and the actual response.
    expect(first?.prompt).toContain('triage: outage')
    expect(first?.raw).toContain('the outage seems fine')
  })

  it("fails the contract, not the task, when the app's verify rejects the value", async () => {
    // `verify` is the half of the contract a schema cannot state. An app gets it
    // for free, and it lands on the same predicate production reads.
    const harnesses = () => listActivityHarnesses(layerOf({ [NOTES]: async () => ({ default: triageHarness }) }, ['notes']))
    const sweep = await runEvalSweep('tiny-1b', {
      only: ['notes:triage'],
      restart: true,
      deps: sweepDeps(harnesses, [JSON.stringify({ priority: 'high', why: 'something is broken' })]),
    })

    const first = sweep.cases[0]
    expect(first?.contractHeld).toBe(false)
    expect(first?.task).toBe('unscored')
    // It is repairable, so the sweep sent the repair turn — the number that
    // separates a usable small model from an unusable one.
    expect(first?.repairs).toBeGreaterThan(0)
  })
})

// ── The deprecated spelling ──────────────────────────────────────────────────

describe('the workbench alias', () => {
  it('still accepts a workbench definition under the old name', () => {
    // `apps/<slug>/harness.ts` files and stored `workbench_harness_defs` rows
    // were written against `defineHarness`. This test is mostly a COMPILE-time
    // assertion: the two contracts are disjoint, so the overload that keeps the
    // old spelling working resolves without ambiguity, and the day it stops
    // doing so is the day third-party apps stop building.
    const wb: WorkbenchHarnessDefinition = defineHarness({
      slug: 'notes-cli',
      label: 'Notes CLI',
      auth: 'gateway',
      invoke: 'npx -y notes-cli run --model <model> "<task>"',
      guide: 'One-shot per task; read the JSON result.',
    })
    expect(wb.slug).toBe('notes-cli')
    expect(defineWorkbenchHarness(wb)).toBe(wb)
  })
})
