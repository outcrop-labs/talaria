// THE APP LAYER of the activity registry, driven through its real surfaces:
// the harness is built with the `defineHarness` an app author imports from
// '@talaria/sdk/server', the checks are the ones the api holds its builtins
// to, and the replay is the real runner. Only the edges an app layer cannot
// have in a test are injected — the module loader and the enabled set — plus
// a scripted transport in place of a model.
import { readFileSync } from 'node:fs'
import { describe, expect, it, vi } from 'vitest'
import { z } from 'zod'
import { defineHarness } from '@/sdk/server'
import type { HarnessDeps } from '@/server/harness/run'
import type { TransportReply } from '@/server/harness/transport'
import { appInScope } from '@/server/app-scope'
import {
  BUILTIN_HARNESS_IDS,
  checkAppHarness,
  checkAppHarnessesOf,
  listAppHarnesses,
  onceEach,
  refreshAppHarnessReports,
  replayAppCase,
  wireOf,
  type AppHarness,
  type AppHarnessLayer,
  type AppHarnessReport,
} from './app-harnesses'

// ── A fixture app ────────────────────────────────────────────────────────────

const TRIAGE = z.object({ severity: z.enum(['low', 'medium', 'high']), reason: z.string() })
type Triage = z.infer<typeof TRIAGE>

/** What an app author writes — the docs/sdk/harnesses.md example. */
const triage = defineHarness<{ subject: string }, Triage>({
  id: 'support:triage',
  label: 'Support triage',
  job: 'Grades an inbound support message so the queue can order itself.',
  requires: ['json', 'instruction-following'],
  floor: { capabilities: [], refuseBelow: false, note: 'Runs on any model that can return JSON.' },
  model: {},
  render: (input) => [{ role: 'user', content: `grade: ${input.subject}` }],
  output: { kind: 'json', schema: TRIAGE },
  onFailure: 'null',
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  evals: [
    { name: 'an outage grades high', input: { subject: 'Checkout is down' }, check: (v) => (v.severity === 'high' ? null : `graded "${v.severity}"`), band: 'easy' },
    { name: 'a typo grades low', input: { subject: 'Typo on the pricing page' }, check: (v) => (v.severity === 'low' ? null : `graded "${v.severity}"`) },
    { name: 'a fixture that cannot ask', input: { subject: 'x' }, check: () => ({ gap: 'the queue was empty, so ordering was unanswerable' }) },
    {
      name: 'a check that throws',
      input: { subject: 'y' },
      check: () => {
        throw new Error('boom')
      },
    },
  ],
})

/** The same harness shape, written the way an app that skipped
 *  `defineHarness` would — no derived floor. */
const raw = (over: Record<string, unknown>): Record<string, unknown> => ({
  ...triage,
  floor: { capabilities: [], refuseBelow: false, note: 'Runs on any model that can return JSON.' },
  ...over,
})

const layerOf = (
  apps: Record<string, Record<string, () => Promise<unknown>>>,
  enabled: string[] = Object.keys(apps),
  warn: (message: string) => void = vi.fn(),
): Partial<AppHarnessLayer> => ({
  enabled: async () => enabled.map((slug) => ({ slug, name: slug.toUpperCase() })),
  files: async (slug) =>
    Object.entries(apps[slug] ?? {})
      .sort(([a], [b]) => (a < b ? -1 : 1))
      .map(([file, load]) => ({ file, load })),
  warn,
})

// ── The builtins the app layer may not take ──────────────────────────────────

describe('BUILTIN_HARNESS_IDS', () => {
  it("is the api's builtin registry, in its order", () => {
    // EXPECTED_IDS is what the Rust registry's own test holds its builtins
    // to; read the source rather than restate it, so a harness added there
    // fails this until it is reserved here too.
    const src = readFileSync(new URL('../../../api/crates/talaria-harness-defs/src/registry.rs', import.meta.url), 'utf8')
    const start = src.indexOf('const EXPECTED_IDS')
    expect(start).toBeGreaterThan(-1)
    const body = src.slice(start, src.indexOf('];', start))
    const ids = [...body.matchAll(/"([^"]+)"/g)].map((m) => m[1])
    expect(ids.length).toBeGreaterThan(30)
    expect([...BUILTIN_HARNESS_IDS]).toEqual(ids)
  })
})

// ── The checks ───────────────────────────────────────────────────────────────

describe('checkAppHarness', () => {
  it('accepts a harness the SDK built, with the JSON floor derived', () => {
    const checked = checkAppHarness(triage)
    expect(checked.ok).toBe(true)
    if (!checked.ok) return
    expect(checked.def.floor.capabilities).toEqual(['json'])
    expect(checked.def.floor.refuseBelow).toBe(true)
  })

  it('derives the JSON floor for a definition written without defineHarness', () => {
    const checked = checkAppHarness(raw({}))
    expect(checked.ok).toBe(true)
    if (checked.ok) expect(checked.def.floor).toMatchObject({ capabilities: ['json'], refuseBelow: true })
  })

  /** Every way a module can be wrong, and the sentence an admin reads for it.
   *  The list is the document: a new rule adds its row here. */
  const BAD: Array<[string, unknown, RegExp]> = [
    ['no default export', undefined, /does not default-export a harness/],
    ['a string', 'harness', /does not default-export a harness/],
    ['no id', raw({ id: '' }), /no id/],
    ['an id with a space', raw({ id: 'support triage' }), /may use only letters/],
    ['no label', raw({ label: '  ' }), /no label/],
    ['no job', raw({ job: undefined }), /no job line/],
    ['no render', raw({ render: undefined }), /no render function/],
    ['no model spec', raw({ model: undefined }), /no model spec/],
    ['a floor with no refusal decision', raw({ floor: { capabilities: [], note: 'x' } }), /floor is not/],
    ['an unknown output kind', raw({ output: { kind: 'yaml' } }), /neither text nor json/],
    ['a json output with no schema', raw({ output: { kind: 'json', schema: {} } }), /no schema to parse with/],
    ['no failure policy', raw({ onFailure: 'shrug' }), /onFailure/],
    ['fixtures that are not a list', raw({ evals: { name: 'x' } }), /evals is not a list/],
    ['a fixture with no check', raw({ evals: [{ name: 'x', input: {} }] }), /no name, no input or no check/],
    ['a guard rule that does not exist', raw({ guard: { rules: ['secret_leak', 'not_a_rule'] } }), /guard rule "not_a_rule" does not exist/],
    ['a floor outside requires', raw({ output: { kind: 'text' }, floor: { capabilities: ['search'], refuseBelow: true, note: 'x' } }), /refuses on "search" without requiring it/],
    ['a json harness that does not require json', raw({ requires: ['instruction-following'] }), /add "json" to requires/],
    ['a floor that never refuses', raw({ output: { kind: 'text' }, requires: ['search'], floor: { capabilities: ['search'], refuseBelow: false, note: 'x' } }), /never refuses/],
    ['an unknown capability', raw({ requires: ['json', 'telepathy'] }), /"telepathy" is not a capability/],
    ['a widening on nothing', raw({ widen: { requires: [], note: 'x' } }), /widens on nothing/],
    ['a widening with no reason', raw({ widen: { requires: ['tools'], note: ' ' } }), /widening has no note/],
    ['a temperature out of range', raw({ temperature: 1.5 }), /temperature 1.5/],
    ['an unknown tool policy', raw({ tools: 'all' }), /tool policy/],
    ['two fixtures with one name', raw({ evals: [triage.evals![0], triage.evals![0]] }), /two fixtures are named/],
    ['an empty floor note', raw({ floor: { capabilities: [], refuseBelow: false, note: '' } }), /no note/],
    // Every field wireOf puts on the wire is checked here: one it could not
    // carry used to cost every app's harnesses, not this one's.
    ['a repair of NaN', raw({ output: { kind: 'json', schema: TRIAGE, repair: Number.NaN } }), /repair NaN is not a whole number of turns/],
    ['a repair of Infinity', raw({ output: { kind: 'json', schema: TRIAGE, repair: Number.POSITIVE_INFINITY } }), /repair Infinity/],
    ['a negative repair', raw({ output: { kind: 'json', schema: TRIAGE, repair: -1 } }), /repair -1/],
    ['a fractional repair', raw({ output: { kind: 'json', schema: TRIAGE, repair: 1.5 } }), /repair 1.5/],
    ['a repair past what a u32 holds', raw({ output: { kind: 'json', schema: TRIAGE, repair: 5_000_000_000 } }), /from 0 to 5/],
    ['a repair that is not a number', raw({ output: { kind: 'json', schema: TRIAGE, repair: '1' } }), /repair 1 is not/],
    ['a model chain that is not a list', raw({ model: { chain: 3 } }), /model chain is not a list/],
    ['a model chain of numbers', raw({ model: { chain: [1, 2] } }), /model chain is not a list/],
    ['a model pin that is not a name', raw({ model: { pin: 7 } }), /model pin/],
    ['a model role that is not a name', raw({ model: { role: {} } }), /model role/],
    ['a suppliable that is not a list', raw({ floor: { capabilities: [], refuseBelow: false, note: 'x', suppliable: 3 } }), /suppliable/],
    ['a widening whose requires is not a list', raw({ widen: { requires: 5, note: 'x' } }), /widens on nothing/],
  ]

  it.each(BAD)('refuses %s', (_why, value, sentence) => {
    const checked = checkAppHarness(value)
    expect(checked.ok).toBe(false)
    if (!checked.ok) expect(checked.problems.join('; ')).toMatch(sentence)
  })

  it('accepts the repair counts and model specs the wire carries', () => {
    for (const repair of [0, 1, 5]) expect(checkAppHarness(raw({ output: { kind: 'json', schema: TRIAGE, repair } })).ok).toBe(true)
    expect(checkAppHarness(raw({ model: { pin: 'titler', role: 'utility', chain: ['pin', 'role'] } })).ok).toBe(true)
    expect(checkAppHarness(raw({ model: { pin: null, role: null, chain: null } })).ok).toBe(true)
  })

  it('reports a definition whose checking throws, rather than throwing', () => {
    const hostile = raw({})
    Object.defineProperty(hostile, 'requires', {
      enumerable: true,
      get: () => {
        throw new Error('a getter that throws')
      },
    })
    const checked = checkAppHarness(hostile)
    expect(checked).toMatchObject({ ok: false, problems: ['it could not be checked: a getter that throws'] })
  })
})

// ── The listing ──────────────────────────────────────────────────────────────

describe('listAppHarnesses', () => {
  it('lists an enabled app’s harnesses and reports the one it skipped, never failing the rest', async () => {
    const warn = vi.fn()
    const broken = raw({ id: 'support:broken', guard: { rules: ['nope'] } })
    const listing = await listAppHarnesses(
      layerOf(
        {
          support: {
            'triage.ts': async () => ({ default: triage }),
            'broken.ts': async () => ({ default: broken }),
            'throws.ts': async () => {
              throw new Error('top-level await rejected')
            },
          },
        },
        ['support'],
        warn,
      ),
    )
    expect(listing.harnesses.map((h) => [h.slug, h.appName, h.file, h.def.id])).toEqual([['support', 'SUPPORT', 'triage.ts', 'support:triage']])
    expect(listing.problems).toEqual([
      { app: 'support', file: 'broken.ts', id: 'support:broken', reason: 'guard rule "nope" does not exist' },
      { app: 'support', file: 'throws.ts', id: null, reason: 'it failed to import: top-level await rejected' },
    ])
    expect(warn).toHaveBeenCalledTimes(2)
  })

  it('follows the enabled set', async () => {
    const listing = await listAppHarnesses(layerOf({ support: { 'triage.ts': async () => ({ default: triage }) } }, []))
    expect(listing.harnesses).toEqual([])
    expect(listing.problems).toEqual([])
  })

  it("refuses a builtin's id and a second claim on one id — the first app, by slug, keeps it", async () => {
    const titler = raw({ id: 'titler' })
    const listing = await listAppHarnesses(
      layerOf({
        zeta: { 'triage.ts': async () => ({ default: triage }) },
        alpha: { 'a.ts': async () => ({ default: triage }), 'b.ts': async () => ({ default: titler }) },
      }),
    )
    expect(listing.harnesses.map((h) => `${h.slug}:${h.file}`)).toEqual(['alpha:a.ts'])
    expect(listing.problems.map((p) => [p.app, p.file, p.reason])).toEqual([
      ['alpha', 'b.ts', '"titler" is a builtin harness\'s id, and an app cannot replace one'],
      ['zeta', 'triage.ts', 'app "alpha" already registers "support:triage"'],
    ])
  })

  it('lists a file the app build could not compile as skipped, with the compiler’s sentence', async () => {
    const listing = await listAppHarnesses({
      ...layerOf({}, ['support']),
      files: async () => [
        { file: 'broken.ts', compileError: 'harnesses/broken.ts:2:0: ERROR: Unexpected end of file', load: () => Promise.reject(new Error('x')) },
        { file: 'triage.ts', load: async () => ({ default: triage }) },
      ],
    })
    expect(listing.harnesses.map((h) => h.file)).toEqual(['triage.ts'])
    expect(listing.problems).toEqual([
      { app: 'support', file: 'broken.ts', id: null, reason: 'it failed to compile: harnesses/broken.ts:2:0: ERROR: Unexpected end of file' },
    ])
  })

  it("never lets one app's malformed harness cost another app's — every listed harness goes on the wire", async () => {
    const listing = await listAppHarnesses(
      layerOf({
        alpha: { 'triage.ts': async () => ({ default: triage }) },
        beta: {
          'chain.ts': async () => ({ default: raw({ id: 'beta:chain', model: { chain: 3 } }) }),
          'nan.ts': async () => ({ default: raw({ id: 'beta:nan', output: { kind: 'json', schema: TRIAGE, repair: Number.NaN } }) }),
          'huge.ts': async () => ({ default: raw({ id: 'beta:huge', output: { kind: 'json', schema: TRIAGE, repair: 2 ** 33 } }) }),
        },
      }),
    )
    expect(listing.harnesses.map((h) => h.def.id)).toEqual(['support:triage'])
    expect(listing.problems.map((p) => p.file)).toEqual(['chain.ts', 'huge.ts', 'nan.ts'])
    // What the route answers: it maps every listed harness through wireOf,
    // and the api decodes `repair` as a u32.
    const wire = JSON.parse(JSON.stringify(listing.harnesses.map(wireOf))) as Array<{ repair: unknown }>
    expect(wire.every((w) => Number.isInteger(w.repair) && (w.repair as number) >= 0 && (w.repair as number) <= 0xffff_ffff)).toBe(true)
  })

  it('warns once per distinct skip, however often the api lists', async () => {
    const sink = vi.fn()
    const warn = onceEach(sink)
    const layer = layerOf(
      {
        support: {
          'triage.ts': async () => ({ default: triage }),
          'broken.ts': async () => ({ default: raw({ id: 'support:broken', guard: { rules: ['nope'] } }) }),
        },
      },
      ['support'],
      warn,
    )
    for (let i = 0; i < 5; i++) await listAppHarnesses(layer)
    expect(sink).toHaveBeenCalledTimes(1)
    expect(sink.mock.calls[0]![0]).toContain('harnesses/broken.ts skipped — guard rule "nope" does not exist')
    // A different problem with the same file is news, and is logged.
    warn('app "support": harnesses/broken.ts skipped — something else')
    expect(sink).toHaveBeenCalledTimes(2)
  })

  it('reports an app whose harness modules cannot even be listed, and keeps the others', async () => {
    const listing = await listAppHarnesses({
      ...layerOf({ good: { 'triage.ts': async () => ({ default: triage }) } }, ['bad', 'good']),
      files: async (slug) => {
        if (slug === 'bad') throw new Error('harnesses.js is not a module')
        return [{ file: 'triage.ts', load: async () => ({ default: triage }) }]
      },
    })
    expect(listing.harnesses.map((h) => h.slug)).toEqual(['good'])
    expect(listing.problems[0]).toMatchObject({ app: 'bad', reason: expect.stringContaining('not a module') })
  })
})

describe('checkAppHarnessesOf — the load probe’s report', () => {
  it('checks an app that is not enabled yet against the ones that are', async () => {
    const layer = layerOf(
      {
        crm: { 'triage.ts': async () => ({ default: triage }) },
        support: { 'triage.ts': async () => ({ default: triage }), 'other.ts': async () => ({ default: raw({ id: 'support:other' }) }) },
      },
      ['crm'],
    )
    // "crm" sorts first and is on, so it keeps the id.
    expect(await checkAppHarnessesOf('support', layer)).toEqual({
      registered: ['support:other'],
      problems: ['harnesses/triage.ts: app "crm" already registers "support:triage"'],
    })
  })

  it('asks nothing of the enabled set for an app that ships no harness', async () => {
    const enabled = vi.fn(async () => [])
    expect(await checkAppHarnessesOf('plain', { enabled, files: async () => [] })).toEqual({ registered: [], problems: [] })
    expect(enabled).not.toHaveBeenCalled()
  })
})

describe('refreshAppHarnessReports — every enabled app’s report, when the set changes', () => {
  const apps = {
    zeta: { 'triage.ts': async () => ({ default: triage }) },
    alpha: { 'triage.ts': async () => ({ default: triage }), 'other.ts': async () => ({ default: raw({ id: 'alpha:other' }) }) },
  }
  const refresh = async (enabled: string[]) => {
    const written = new Map<string, AppHarnessReport>()
    await refreshAppHarnessReports(layerOf(apps, enabled), (slug, report) => written.set(slug, report))
    return written
  }

  it('moves a contested id to the app that sorts first, and shows the skip on the app that lost it', async () => {
    // zeta alone holds the id.
    expect(await refresh(['zeta'])).toEqual(new Map([['zeta', { registered: ['support:triage'], problems: [] }]]))
    // alpha is enabled and sorts first: it takes the id, and zeta's report
    // says so — it used to keep "1 harness" from its own probe.
    expect(await refresh(['alpha', 'zeta'])).toEqual(
      new Map([
        ['alpha', { registered: ['alpha:other', 'support:triage'], problems: [] }],
        ['zeta', { registered: [], problems: ['harnesses/triage.ts: app "alpha" already registers "support:triage"'] }],
      ]),
    )
    // alpha is switched off: zeta has it back, and alpha's report is left as
    // it was (Manage → Apps says a disabled app's harnesses are not registered).
    expect(await refresh(['zeta'])).toEqual(new Map([['zeta', { registered: ['support:triage'], problems: [] }]]))
  })

  it('reads the enabled set once', async () => {
    const enabled = vi.fn(async () => [{ slug: 'zeta', name: 'Zeta' }])
    await refreshAppHarnessReports({ ...layerOf(apps), enabled }, () => {})
    expect(enabled).toHaveBeenCalledTimes(1)
  })
})

// ── The wire ─────────────────────────────────────────────────────────────────

const registered = async (): Promise<AppHarness> => {
  const listing = await listAppHarnesses(layerOf({ support: { 'triage.ts': async () => ({ default: triage }) } }))
  return listing.harnesses[0]!
}

describe('wireOf', () => {
  it('is data the api can read — no code crosses', async () => {
    const wire = wireOf(await registered())
    expect(wire).toEqual({
      id: 'support:triage',
      label: 'Support triage',
      job: 'Grades an inbound support message so the queue can order itself.',
      source: 'app:support',
      app: 'support',
      appName: 'SUPPORT',
      file: 'triage.ts',
      requires: ['json', 'instruction-following'],
      floor: { capabilities: ['json'], refuseBelow: true, note: 'Runs on any model that can return JSON.' },
      outputKind: 'json',
      repair: 1,
      verifies: false,
      tools: 'none',
      guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
      widen: null,
      temperature: null,
      model: { pin: null, role: null, chain: null, slot: 'app:support/support:triage' },
      loop: null,
      evals: [
        { name: 'an outage grades high', band: 'easy' },
        { name: 'a typo grades low', band: 'standard' },
        { name: 'a fixture that cannot ask', band: 'standard' },
        { name: 'a check that throws', band: 'standard' },
      ],
    })
    expect(JSON.parse(JSON.stringify(wire))).toEqual(wire)
  })
})

// ── The replay ───────────────────────────────────────────────────────────────

const GUARD_OFF = { mode: 'off' as const, checks: {}, minConfidence: 1, policedHosts: [], coach: false }

/** The runner's edges with a scripted model: every reply is `text`, usage as
 *  given. Counts calls and what the run tried to FILE, which must be nothing. */
function scripted(text: string, opts: { usage?: TransportReply['usage']; jsonFalse?: boolean } = {}) {
  const filed = { runs: 0, findings: 0, calls: 0, models: [] as string[] }
  const deps: Partial<HarnessDeps> = {
    routing: async (m) => ({ endpoints: ['mock'], upstreamModel: m }),
    transport: async (req) => {
      filed.calls++
      filed.models.push(req.model)
      return { kind: 'gateway', text, toolNames: [], usage: opts.usage === undefined ? { promptTokens: 30, completionTokens: 8 } : opts.usage, contractDropped: false }
    },
    capabilities: async () => (opts.jsonFalse ? { json: { value: false, source: 'probe' } } : {}),
    missingCapabilities: async (_key, asked) => (opts.jsonFalse ? asked.filter((c) => c === 'json') : []),
    guardConfig: async () => GUARD_OFF,
    guardText: async () => [],
    recordFindings: async () => {
      filed.findings++
    },
    recordRun: async () => {
      filed.runs++
    },
  }
  return { deps, filed }
}

describe('replayAppCase', () => {
  it('replays a fixture with the model pinned and grades it with the fixture’s own check', async () => {
    const h = await registered()
    const { deps, filed } = scripted('{"severity":"high","reason":"checkout is down"}')
    const r = await replayAppCase(h, 'an outage grades high', 'mock-small', { deps })
    expect(r).toMatchObject({
      harness: 'support:triage',
      case: 'an outage grades high',
      band: 'easy',
      model: 'mock-small',
      value: { severity: 'high', reason: 'checkout is down' },
      verdict: 'pass',
      reason: null,
      schemaValid: true,
      repairs: 0,
      answered: true,
      refused: false,
      promptTokens: 30,
      completionTokens: 8,
      estimated: false,
    })
    expect(r?.prompt).toContain('grade: Checkout is down')
    expect(filed.models).toEqual(['mock-small'])
    // The run ledger stays off: nothing reached the recorders it would file
    // through, because the replay captures instead.
    expect(filed.runs).toBe(0)
    expect(filed.findings).toBe(0)
  })

  it('fails the model when the fixture rejects the value', async () => {
    const { deps } = scripted('{"severity":"medium","reason":"a typo"}')
    const r = await replayAppCase(await registered(), 'a typo grades low', 'mock-small', { deps })
    expect(r).toMatchObject({ verdict: 'fail', reason: 'graded "medium"', schemaValid: true })
  })

  it('reports a gap — and a check that throws — as the fixture’s, not the model’s', async () => {
    const { deps } = scripted('{"severity":"low","reason":"x"}')
    const h = await registered()
    expect(await replayAppCase(h, 'a fixture that cannot ask', 'm', { deps })).toMatchObject({ verdict: 'gap', reason: 'the queue was empty, so ordering was unanswerable' })
    expect(await replayAppCase(h, 'a check that throws', 'm', { deps })).toMatchObject({ verdict: 'gap', reason: "the fixture's check threw: boom" })
  })

  it('grades nothing when the contract failed, after the repair turn', async () => {
    const { deps, filed } = scripted('not json at all')
    const r = await replayAppCase(await registered(), 'an outage grades high', 'm', { deps })
    expect(r).toMatchObject({ verdict: null, schemaValid: false, repairs: 1, answered: true })
    expect(filed.calls).toBe(2)
    expect(r?.raw).toBe('not json at all')
  })

  it('reports a floor refusal as refused, with no model call', async () => {
    const { deps, filed } = scripted('{"severity":"high","reason":"x"}', { jsonFalse: true })
    const r = await replayAppCase(await registered(), 'an outage grades high', 'm', { deps })
    expect(r).toMatchObject({ refused: true, verdict: null, answered: false })
    expect(filed.calls).toBe(0)
  })

  it('estimates the tokens when the transport reported none', async () => {
    const { deps } = scripted('{"severity":"high","reason":"x"}', { usage: null })
    const r = await replayAppCase(await registered(), 'an outage grades high', 'm', { deps })
    expect(r?.estimated).toBe(true)
    expect(r?.promptTokens).toBeGreaterThan(0)
  })

  it('runs as the app: its calls sign with the app\'s gateway key, under the app\'s fitness stamp', async () => {
    // The transport finds the key by the app in scope (app-scope.ts), never by
    // a caller name; the stamp is what the relay books the replay's spend under.
    const { deps } = scripted('{"severity":"high","reason":"x"}')
    const seen: Array<{ app: string | null; caller: string }> = []
    const base = deps.transport!
    deps.transport = async (req) => {
      seen.push({ app: appInScope(), caller: req.caller })
      return base(req)
    }
    const h = await registered()
    await replayAppCase(h, 'an outage grades high', 'm', { deps })
    expect(seen).toEqual([{ app: h.slug, caller: `app:${h.slug}:fitness:support:triage` }])
    expect(appInScope()).toBeNull()
  })

  it('answers null for a fixture the harness does not declare', async () => {
    expect(await replayAppCase(await registered(), 'no such fixture', 'm')).toBeNull()
  })
})

// ── The tool loop ────────────────────────────────────────────────────────────

const LOOKUP = z.object({ customer: z.string() })

/** A loop harness an app writes: a handler with a simulate, its own MCP tool
 *  with a simulate, and a fixture that asserts what the model DID. */
const handlerSpy = vi.fn(async () => ({ id: 'real' }))
const lookup = defineHarness<{ email: string }, z.infer<typeof LOOKUP>>({
  id: 'support:lookup',
  label: 'Support lookup',
  job: 'Finds the customer, then their orders.',
  requires: ['json', 'tools'],
  floor: { capabilities: [], refuseBelow: false, note: 'Any model that calls tools.' },
  model: {},
  render: (i) => [{ role: 'user', content: `look up ${i.email}` }],
  output: { kind: 'json', schema: LOOKUP },
  onFailure: 'null',
  loop: {
    maxTurns: 4,
    tools: [
      { name: 'find_customer', description: 'Find a customer.', parameters: { type: 'object' }, handler: handlerSpy, simulate: (args) => ({ id: 'sim', email: args.email }) },
      { name: 'recent_orders', description: 'Orders.', parameters: { type: 'object' }, mcp: 'orders_recent', simulate: () => ({ count: 2 }) },
    ],
  },
  evals: [
    {
      name: 'looks the customer up before the orders',
      input: { email: 'ada@example.test' },
      check: (_v, ctx) => (ctx.calledBefore('find_customer', 'recent_orders') ? null : 'read orders before finding the customer'),
    },
  ],
})

describe('a tool loop', () => {
  it('is checked whole at load: one road per tool, real names, a bounded loop', () => {
    expect(checkAppHarness(lookup).ok).toBe(true)
    const tool = { name: 'find', description: 'd', parameters: { type: 'object' }, handler: () => 1 }
    const cases: Array<[string, unknown, RegExp]> = [
      ['no tools', { tools: [] }, /loop offers no tools/],
      ['two roads', { tools: [{ ...tool, mcp: 'x' }] }, /exactly one of handler, mcp or org/],
      ['no road', { tools: [{ name: 'find', description: 'd', parameters: { type: 'object' } }] }, /exactly one of handler, mcp or org/],
      ['a bad org', { tools: [{ name: 'find', description: 'd', parameters: {}, org: 'no-slash' }] }, /'<server>\/<tool>'/],
      ['a bad name', { tools: [{ ...tool, name: 'has space' }] }, /letters, digits, _ and -/],
      ['a repeated name', { tools: [tool, tool] }, /two loop tools are named "find"/],
      ['no description', { tools: [{ ...tool, description: '' }] }, /no description/],
      ['too many turns', { tools: [tool], maxTurns: 40 }, /maxTurns 40/],
      ['a simulate that is not a function', { tools: [{ ...tool, simulate: 'x' }] }, /simulate is not a function/],
    ]
    for (const [label, loop, why] of cases) {
      const r = checkAppHarness(raw({ loop }))
      expect(r.ok, label).toBe(false)
      if (!r.ok) expect(r.problems.join('; '), label).toMatch(why)
    }
    const both = checkAppHarness(raw({ loop: { tools: [tool] }, tools: 'own' }))
    expect(both.ok).toBe(false)
    const clash = checkAppHarness(raw({ loop: { tools: [tool] }, toolDefs: [{ name: 'find', description: 'd', parameters: {} }] }))
    expect(!clash.ok && clash.problems.join()).toMatch(/also offered in toolDefs/)
  })

  it('goes on the wire as data: the turn bound, and per tool its road and whether it can be dry-run', async () => {
    const { harnesses } = await listAppHarnesses(layerOf({ support: { 'lookup.ts': async () => ({ default: lookup }) } }))
    expect(wireOf(harnesses[0]!).loop).toEqual({
      maxTurns: 4,
      tools: [
        { name: 'find_customer', via: 'handler', simulate: true },
        { name: 'recent_orders', via: 'mcp', simulate: true },
      ],
    })
  })

  it('is replayed as a dry run: every tool answers through simulate, and the fixture reads the calls', async () => {
    handlerSpy.mockClear()
    const { harnesses } = await listAppHarnesses(layerOf({ support: { 'lookup.ts': async () => ({ default: lookup }) } }))
    let n = 0
    const turns: TransportReply[] = [
      { kind: 'gateway', text: '', toolNames: ['find_customer'], toolCalls: [{ name: 'find_customer', args: '{"email":"ada@example.test"}', id: 'c1' }], usage: null, contractDropped: false },
      { kind: 'gateway', text: '', toolNames: ['recent_orders'], toolCalls: [{ name: 'recent_orders', args: '{}', id: 'c2' }], usage: null, contractDropped: false },
      { kind: 'gateway', text: '{"customer":"sim"}', toolNames: [], toolCalls: [], usage: null, contractDropped: false },
    ]
    const deps: Partial<HarnessDeps> = {
      ...scripted('').deps,
      transport: async () => turns[Math.min(n++, turns.length - 1)]!,
    }
    const r = await replayAppCase(harnesses[0]!, 'looks the customer up before the orders', 'mock-small', { deps })
    expect(handlerSpy).not.toHaveBeenCalled()
    expect(r).toMatchObject({ verdict: 'pass', schemaValid: true, exhausted: false })
    expect(r?.calls).toEqual([
      { tool: 'find_customer', args: { email: 'ada@example.test' }, result: { id: 'sim', email: 'ada@example.test' }, error: null },
      { tool: 'recent_orders', args: {}, result: { count: 2 }, error: null },
    ])
  })
})
