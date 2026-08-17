import { beforeEach, describe, expect, it, vi } from 'vitest'
import { z } from 'zod'

// Every scorer here is driven from a RECORDED REPLY, and that is the whole
// design of the file rather than a testing convenience. These probes are the
// first production writer of `value: true` in a capability record, and a wrong
// fact does not expire — so the thing that has to be held still is what a
// specific bad reply from a specific weak model scores, not whether some model
// somewhere passes. The gateway, the database and the clock are all injected.
const { state } = vi.hoisted(() => ({
  state: {
    endpoints: [] as Array<Record<string, unknown>>,
    upstreamModel: 'qwen3-14b',
    personaKeys: [] as string[],
  },
}))

// Partial: `harness/run.ts` and `harness/model.ts` pull half of this module in
// at import time, and replacing the lot would fail before a single case ran.
// Only the two functions the probes actually read are scripted.
vi.mock('../llm-gateway', async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  routingFor: (): Promise<unknown> => Promise.resolve({ endpoints: state.endpoints, upstreamModel: state.upstreamModel }),
  gatewayPulse: () => ({ requests: 12, errors: 1, p50: 140, p95: 620 }),
}))

vi.mock('../harness/persona', () => ({
  personaCapabilityKeys: (): Promise<string[]> => Promise.resolve(state.personaKeys),
}))

import {
  CODE_TASKS,
  DEFAULT_MAX_CONTEXT_TOKENS,
  MIN_LONG_CONTEXT_TOKENS,
  PROBES,
  citationProblem,
  dateDriftDays,
  defaultDeps,
  estimateProbes,
  extractCode,
  haystack,
  needleLine,
  quoteAppears,
  rateOf,
  readSearchReply,
  runCodeTask,
  runProbes,
  runnerAsk,
  runnerToolAsk,
  scoreCode,
  scoreInstruction,
  scoreJson,
  scoreJsonStrict,
  scoreLongContext,
  scoreSearch,
  scoreToolSelect,
  scoreTools,
  toolCallProblem,
  type Attempt,
  type AskSpec,
  type ProbeDeps,
  type ProbeId,
  type ProbeOutcome,
  type ToolAskSpec,
  type ToolAttempt,
  type Trial,
} from './probes'
import { CAPABILITIES, type Capability, type CapabilityFact, type CapabilityKey } from '../harness/capability'
import type { TransportReply, TransportRequest } from '../harness/run'

// ── Helpers ──────────────────────────────────────────────────────────────────

const trial = (over: Partial<Trial> = {}): Trial => ({ name: 'trial', ok: true, note: '', raw: null, ...over })
const pass = (name = 'trial'): Trial => trial({ name, ok: true })
const fail = (name = 'trial', note = 'failed'): Trial => trial({ name, ok: false, note })
const unknown = (name = 'trial'): Trial => trial({ name, ok: null, note: 'inconclusive' })

const attempt = (over: Partial<Attempt> = {}): Attempt => ({
  raw: '',
  transportError: null,
  jsonRequested: true,
  contractDropped: false,
  contractHeld: true,
  ...over,
})

const endpoint = (name: string, over: Record<string, unknown> = {}): Record<string, unknown> => ({
  name,
  contextLength: null,
  modelPrices: {},
  autoPrices: {},
  priceInPerMtok: null,
  priceOutPerMtok: null,
  ...over,
})

interface Written {
  key: CapabilityKey
  cap: Capability
  fact: CapabilityFact
}

/** A probe run with every edge injected: no gateway, no database, no network,
 *  no clock. `ask` is keyed on the id prefix each probe passes. */
function harness(opts: {
  reply?: (spec: AskSpec) => Partial<Attempt>
  /** What the model calls when this probe offers it tools. Default: nothing,
   *  which is a failed trial rather than a missing one. */
  tools?: (spec: ToolAskSpec) => Partial<ToolAttempt>
  over?: Partial<ProbeDeps>
}): { deps: Partial<ProbeDeps>; written: Written[]; asked: string[] } {
  const written: Written[] = []
  const asked: string[] = []
  const deps: Partial<ProbeDeps> = {
    ask: (spec) => {
      asked.push(spec.id)
      return Promise.resolve(attempt(opts.reply?.(spec) ?? {}))
    },
    askWithTools: (spec) => {
      asked.push(spec.id)
      return Promise.resolve({ toolCalls: [], transportError: null, ...opts.tools?.(spec) })
    },
    // The tool channel is open on a gateway model, which is the ordinary case.
    // The tests that care about a fleet candidate override this.
    offersToolDefinitions: () => Promise.resolve(true),
    askWithImages: null,
    contextWindow: () => Promise.resolve(null),
    advertises: () => Promise.resolve(false),
    fetchText: () => Promise.resolve(null),
    record: (key, cap, fact) => {
      written.push({ key, cap, fact })
      return Promise.resolve()
    },
    now: () => Date.parse('2026-08-06T09:00:00Z'),
    price: () => Promise.resolve(null),
    // NOTHING MEASURED YET is the default, so every existing test keeps asking
    // the model. The reuse path is opted into by the tests that are about it.
    measured: () => Promise.resolve(null),
    ...opts.over,
  }
  return { deps, written, asked }
}

const outcomeOf = (report: { results: Array<{ id: ProbeId; outcome: ProbeOutcome }> }, id: ProbeId): ProbeOutcome => {
  const hit = report.results.find((r) => r.id === id)
  if (!hit) throw new Error(`no result for probe ${id}`)
  return hit.outcome
}

beforeEach(() => {
  state.endpoints = [endpoint('pl-main')]
  state.upstreamModel = 'qwen3-14b'
  state.personaKeys = []
})

// ── The registry itself ──────────────────────────────────────────────────────

describe('the probe registry', () => {
  it('names every Capability exactly once — a capability with no probe is unmeasurable', () => {
    // ASSERTED AGAINST THE UNION, not against a copy of it. A tenth member of
    // `Capability` with no probe is a fact nothing can ever establish: the
    // gateway only writes those it learns from a 400, so an unprobed capability
    // can only ever be unknown or false, and any harness that `requires` it can
    // never reach Ready. That has to fail here rather than in six months.
    expect([...PROBES.map((p) => p.id)].sort()).toEqual([...CAPABILITIES].sort())
  })

  it('declares a call count and a token size for every probe, because the estimate is shown before spending', () => {
    for (const p of PROBES) {
      expect(p.calls, p.id).toBeGreaterThan(0)
      expect(p.promptTokens, p.id).toBeGreaterThan(0)
      expect(p.completionTokens, p.id).toBeGreaterThan(0)
      expect(p.claim.length, p.id).toBeGreaterThan(10)
    }
  })
})

// ── rateOf: the inconclusive rule ────────────────────────────────────────────

describe('rateOf', () => {
  it('ignores inconclusive trials rather than counting them as failures', () => {
    expect(rateOf([pass(), fail(), unknown()])).toBe(0.5)
  })

  it('answers null when nothing was conclusive, which is what suppresses the write', () => {
    expect(rateOf([unknown(), unknown()])).toBeNull()
    expect(rateOf([])).toBeNull()
  })
})

// ── json ─────────────────────────────────────────────────────────────────────

describe('scoreJson', () => {
  const protocol = { requested: true, dropped: false }

  it('records true when every JSON-mode call returned a usable object', () => {
    const v = scoreJson([pass('a'), pass('b'), pass('c')], protocol)
    expect(v).toMatchObject({ value: true, score: 1 })
    expect(v?.detail).toContain('response_format')
  })

  it('records the MODEL as capable on a contract drop when every reply parsed', () => {
    // The gateway learned an upstream 400 on `response_format`, pre-stripped it,
    // the call succeeded, and every reply was still JSON because the prompt
    // anchor asked for it.
    //
    // THIS USED TO RECORD `false`, and that became load-bearing the moment a
    // JSON harness put `json` in its floor: a self-hosted server with no
    // response_format support would have had all nine structured harnesses
    // declared unfit for models that produce perfect JSON. The endpoint's gap is
    // tracked where it belongs — `contractDropped` and the learned-param ratchet
    // — and this fact is about the MODEL.
    //
    // Answering from the prompt alone is the HARDER question, so passing it
    // three for three is a stronger result than honoring the parameter, not a
    // weaker one. The detail still names the drop so nobody reads the verdict as
    // "this endpoint constrains decoding".
    const v = scoreJson([pass('a'), pass('b'), pass('c')], { requested: true, dropped: true })
    expect(v).toMatchObject({ value: true, score: 1 })
    expect(v?.detail).toContain('dropped response_format')
    expect(v?.detail).toContain('from the prompt alone')
  })

  it('records false with the observed rate when replies did not parse', () => {
    const v = scoreJson([pass('a'), fail('b', 'trailing prose after the object'), fail('c', 'two objects')], protocol)
    expect(v).toMatchObject({ value: false })
    expect(v?.score).toBeCloseTo(1 / 3)
    expect(v?.detail).toContain('trailing prose after the object')
  })

  it('writes nothing when JSON mode was never requested - we cannot record what we did not test', () => {
    expect(scoreJson([pass(), pass(), pass()], { requested: false, dropped: false })).toBeNull()
  })
})

describe('scoreJsonStrict', () => {
  it('accepts 4 of 5, because the runner has a repair turn behind it', () => {
    const v = scoreJsonStrict([pass('1'), pass('2'), pass('3'), pass('4'), fail('5', 'summary was 90 characters')])
    expect(v).toMatchObject({ value: true })
    expect(v?.score).toBeCloseTo(0.8)
  })

  it('rejects 3 of 5 and names the first failure', () => {
    const v = scoreJsonStrict([pass('1'), pass('2'), pass('3'), fail('4', 'unescaped newline in summary'), fail('5', 'items was a string')])
    expect(v).toMatchObject({ value: false })
    expect(v?.detail).toContain('unescaped newline in summary')
  })
})

// ── tools / tool-select ──────────────────────────────────────────────────────

describe('toolCallProblem', () => {
  it('accepts a single correct call with the required argument', () => {
    expect(toolCallProblem([{ name: 'get_weather', args: '{"city":"Lisbon"}' }], 'get_weather', ['city'])).toBeNull()
  })

  it('rejects a prose answer', () => {
    expect(toolCallProblem([], 'get_weather', ['city'])).toContain('prose')
  })

  it('rejects the wrong tool', () => {
    expect(toolCallProblem([{ name: 'send_email', args: '{}' }], 'get_weather', [])).toBe('called send_email instead of get_weather')
  })

  it('rejects a shotgun that calls everything', () => {
    const calls = [
      { name: 'get_weather', args: '{}' },
      { name: 'send_email', args: '{}' },
    ]
    expect(toolCallProblem(calls, 'get_weather', [])).toContain('called 2 tools')
  })

  it('rejects arguments that are not JSON, and arguments that are missing', () => {
    expect(toolCallProblem([{ name: 'get_weather', args: 'city=Lisbon' }], 'get_weather', ['city'])).toContain('not JSON')
    expect(toolCallProblem([{ name: 'get_weather', args: '{"town":"Lisbon"}' }], 'get_weather', ['city'])).toContain('without city')
  })
})

describe('scoreToolSelect', () => {
  it('needs all four - a model that picks right 3 of 4 has not earned the Inbox widening', () => {
    const v = scoreToolSelect([pass('weather'), pass('email'), pass('currency'), fail('ticket', 'called send_email instead of create_ticket')])
    expect(v).toMatchObject({ value: false })
    expect(v?.score).toBeCloseTo(0.75)
    expect(v?.detail).toContain('widening needs all of them')
  })

  it('records true only at 4 of 4', () => {
    expect(scoreToolSelect([pass('a'), pass('b'), pass('c'), pass('d')])).toMatchObject({ value: true, score: 1 })
  })
})

describe('scoreTools', () => {
  it('is pass/fail on the single offered tool', () => {
    expect(scoreTools([pass()])).toMatchObject({ value: true })
    expect(scoreTools([fail('t', 'answered in prose')])).toMatchObject({ value: false })
  })
})

// ── instruction-following ────────────────────────────────────────────────────

describe('scoreInstruction', () => {
  it('records true only when every exact-output instruction came back verbatim', () => {
    expect(scoreInstruction([pass(), pass(), pass()])).toMatchObject({ value: true, score: 1 })
  })

  it('records false at 2 of 3 and quotes what came back instead', () => {
    const v = scoreInstruction([pass('exactly OK'), fail('exactly three words', 'answered "Sure! red green blue"'), pass('exactly one digit')])
    expect(v).toMatchObject({ value: false })
    expect(v?.detail).toContain('Sure! red green blue')
  })
})

// ── search ───────────────────────────────────────────────────────────────────

describe('citationProblem', () => {
  it('accepts a real absolute URL', () => {
    expect(citationProblem('https://news.ycombinator.com/item?id=1')).toBeNull()
  })

  it('rejects a relative link, a bare host, and the placeholder hosts a model reaches for when inventing', () => {
    expect(citationProblem('/docs/index.html')).toContain('not an absolute URL')
    expect(citationProblem('https://intranet/page')).toContain('no real host')
    expect(citationProblem('https://example.com/story')).toContain('placeholder host')
  })
})

describe('dateDriftDays', () => {
  const now = Date.parse('2026-08-06T09:00:00Z')

  it('measures the gap in whole days', () => {
    expect(dateDriftDays('2026-08-06', now)).toBe(0)
    expect(dateDriftDays('2026-08-05', now)).toBe(1)
    expect(dateDriftDays('2025-04-01', now)).toBeGreaterThan(400)
  })

  it('answers null for anything that is not a plain ISO date', () => {
    expect(dateDriftDays('August 6th', now)).toBeNull()
    expect(dateDriftDays('2026-13-45', now)).toBeNull()
  })
})

describe('quoteAppears', () => {
  const page = '<article>\n  <p>The council <b>approved</b> the new ferry timetable on Tuesday evening.</p>\n</article>'

  it('matches through tags and line wrapping, which is the only difference between markup and rendered text', () => {
    expect(quoteAppears('The council approved the new ferry timetable on Tuesday evening.', page)).toBe(true)
  })

  it('rejects a sentence that is not on the page', () => {
    expect(quoteAppears('The council rejected the new ferry timetable on Tuesday evening.', page)).toBe(false)
  })

  it('rejects a quote too short to be evidence of anything', () => {
    expect(quoteAppears('approved', page)).toBe(false)
  })
})

describe('scoreSearch', () => {
  it('records true off ONE attempt that both named today and quoted a page we fetched', () => {
    const trials = [pass('a / date'), pass('a / citation'), fail('b / date', 'said today is 2024-06-01'), unknown('b / citation')]
    expect(scoreSearch(trials)).toMatchObject({ value: true })
  })

  it('writes NOTHING for a verified quote the model could not date - that is a good memory, not a search', () => {
    // deepseek-v4-pro's real shape, and it used to earn a permanent `search:
    // true` on an endpoint that returns no citations at all. Research then ran
    // its search stages natively on a model that never searched.
    expect(scoreSearch([fail('a / date', 'said today is 2024-06-01'), pass('a / citation')])).toBeNull()
  })

  it('will not pair a passing quote with a passing date from a DIFFERENT reply', () => {
    // Two half-successes are not one success: the attempt that dated correctly
    // cited nothing checkable, and the attempt that quoted a real page did not
    // know what day it was.
    expect(scoreSearch([pass('a / date'), unknown('a / citation'), fail('b / date', 'said today is 2024-06-01'), pass('b / citation')])).toBeNull()
  })

  it('records false only when the model could not name today, which needs no network of ours', () => {
    const v = scoreSearch([
      fail('a / date', 'said today is 2024-06-01'),
      fail('a / citation', 'the citation is a placeholder host: example.com'),
      fail('b / date', 'said today is 2024-06-01'),
      fail('b / citation', 'the citation is a placeholder host: example.com'),
    ])
    expect(v).toMatchObject({ value: false })
    expect(v?.detail).toContain('2024-06-01')
  })

  it('writes NOTHING when only the quote check failed - a 403 on a cited page must never refuse a working search model forever', () => {
    expect(
      scoreSearch([
        pass('a / date'),
        fail('a / citation', 'the quoted sentence is not on https://news.example.org/x'),
        pass('b / date'),
        unknown('b / citation'),
      ]),
    ).toBeNull()
  })
})

describe('readSearchReply', () => {
  it('recovers the object out of a fenced, chatty reply', () => {
    const raw = 'Sure!\n```json\n{"date":"2026-08-06","url":"https://a.example/x","quote":"a sentence"}\n```\nHope that helps.'
    expect(readSearchReply(raw)).toEqual({ date: '2026-08-06', url: 'https://a.example/x', quote: 'a sentence' })
  })

  it('answers null on prose and on a wrong shape', () => {
    expect(readSearchReply('I could not find anything.')).toBeNull()
    expect(readSearchReply('{"date":"2026-08-06"}')).toBeNull()
  })
})

// ── long-context ─────────────────────────────────────────────────────────────

describe('haystack', () => {
  it('plants the needle at the requested depth of the filler', () => {
    const needle = needleLine('GRANITE-FOX-7731')
    const text = haystack(1_100, needle, 0.9)
    const lines = text.split('\n')
    const at = lines.indexOf(needle)
    expect(at).toBeGreaterThan(0)
    expect(at / lines.length).toBeGreaterThan(0.85)
  })

  it('sizes the filler from the token budget it was given', () => {
    const short = haystack(1_100, 'needle', 0.5).length
    const long = haystack(11_000, 'needle', 0.5).length
    expect(long).toBeGreaterThan(short * 8)
  })
})

describe('scoreLongContext', () => {
  it('needs both depths, and says which window was actually tested', () => {
    const both = scoreLongContext([pass('needle at 50%'), pass('needle at 90%')], 25_600)
    expect(both).toMatchObject({ value: true })
    expect(both?.detail).toContain('25,600')
    const half = scoreLongContext([pass('needle at 50%'), fail('needle at 90%', 'the passphrase was not in the reply')], 25_600)
    expect(half).toMatchObject({ value: false })
    expect(half?.score).toBeCloseTo(0.5)
  })
})

// ── code ─────────────────────────────────────────────────────────────────────

const SLUGIFY = CODE_TASKS[0]!
const MERGE = CODE_TASKS[1]!

const GOOD_SLUGIFY = `function slugify(input) {
  return String(input).toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')
}`

const GOOD_MERGE = `function mergeRanges(ranges) {
  const sorted = ranges.map((r) => [r[0], r[1]]).sort((a, b) => a[0] - b[0])
  const out = []
  for (const r of sorted) {
    const last = out[out.length - 1]
    if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1])
    else out.push([r[0], r[1]])
  }
  return out
}`

describe('runCodeTask', () => {
  it('passes a correct function - graded by RUNNING the assertions, never by another model', () => {
    expect(runCodeTask(SLUGIFY, GOOD_SLUGIFY)).toBeNull()
    expect(runCodeTask(MERGE, GOOD_MERGE)).toBeNull()
  })

  it('accepts the two wrappers a model habitually adds: a fence and a leading export', () => {
    expect(runCodeTask(SLUGIFY, '```js\nexport ' + GOOD_SLUGIFY + '\n```')).toBeNull()
  })

  it('accepts a stray debug log rather than failing the model for our bare context', () => {
    const chatty = GOOD_SLUGIFY.replace('return', "console.log('slugifying');\n  return")
    expect(runCodeTask(SLUGIFY, chatty)).toBeNull()
  })

  it('names the exact failing assertion for a near-miss', () => {
    // The classic small-model version: no trimming of the leading/trailing dash.
    const nearly = "function slugify(input) { return String(input).toLowerCase().replace(/[^a-z0-9]+/g, '-') }"
    expect(runCodeTask(SLUGIFY, nearly)).toContain('expected "hello-world"')
  })

  it('fails a function that was never defined, and prose with no code in it', () => {
    expect(runCodeTask(SLUGIFY, 'function slug(x) { return x }')).toContain('no function named slugify')
    expect(runCodeTask(SLUGIFY, '   ')).toContain('returned no code')
  })

  it('fails code that does not parse rather than throwing out of the probe', () => {
    expect(runCodeTask(SLUGIFY, 'function slugify(input) { return input.toLowerCase(')).toContain('did not run')
  })

  it('survives an infinite loop - a wrong regex loop costs a timeout, not a wedged request', () => {
    const spin = 'function slugify(input) { while (true) {} }'
    expect(runCodeTask(SLUGIFY, spin)).toBeTruthy()
  })

  it('is not corrupted by a function that mutates its arguments', () => {
    const mutating = `function mergeRanges(ranges) {
      ranges.sort((a, b) => a[0] - b[0])
      const out = []
      for (const r of ranges) {
        const last = out[out.length - 1]
        if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1])
        else out.push(r)
      }
      return out
    }`
    // The arguments are re-parsed from JSON inside the sandbox, one array per
    // case, so a solution that sorts in place cannot make a later case fail for
    // a reason that has nothing to do with the model.
    expect(runCodeTask(MERGE, mutating)).toBeNull()
  })
})

describe('extractCode', () => {
  it('takes the fenced block when there is one and leaves bare source alone', () => {
    expect(extractCode('Here you go:\n```javascript\nconst a = 1\n```\nDone.')).toBe('const a = 1\n')
    expect(extractCode('const a = 1')).toBe('const a = 1')
  })

  it('strips only a leading export, not the word export inside code', () => {
    expect(extractCode('export function f() { return "export function" }')).toBe('function f() { return "export function" }')
  })
})

describe('scoreCode', () => {
  it('is the fraction of tasks whose function passed every assertion', () => {
    expect(scoreCode([pass('slugify'), pass('mergeRanges')])).toMatchObject({ value: true, score: 1 })
    const half = scoreCode([pass('slugify'), fail('mergeRanges', 'mergeRanges([[1,2],[2,3]]) returned [[1,2],[2,3]]')])
    expect(half).toMatchObject({ value: false })
    expect(half?.detail).toContain('mergeRanges')
  })
})

// ── The driver ───────────────────────────────────────────────────────────────

describe('runProbes', () => {
  it('writes one probe fact per scored probe, keyed endpoint:model', async () => {
    const { deps, written } = harness({ reply: () => ({ raw: '{"name":"talaria","count":3,"ok":true}', contractHeld: true }) })
    const report = await runProbes('qwen3-14b', { ids: ['json'], deps })
    expect(report.keys).toEqual(['pl-main:qwen3-14b'])
    expect(report.wrote).toBe(1)
    expect(written).toHaveLength(1)
    expect(written[0]).toMatchObject({ key: 'pl-main:qwen3-14b', cap: 'json' })
    expect(written[0]?.fact).toMatchObject({ value: true, source: 'probe', score: 1 })
    expect(written[0]?.fact.at).toBe('2026-08-06T09:00:00.000Z')
    expect(written[0]?.fact.detail?.length ?? 0).toBeGreaterThan(10)
  })

  it('REUSES a capability an earlier run already probed instead of buying it again', async () => {
    // The saving that matters on a re-test: nine probes on a model tested last
    // month is nine calls for an answer we already wrote down. A probe fact is a
    // property of an `endpoint:model` and does not go stale on its own.
    const { deps, written, asked } = harness({
      reply: () => ({ raw: '{"name":"talaria","count":3,"ok":true}', contractHeld: true }),
      over: {
        measured: (cap) =>
          Promise.resolve(cap === 'json' ? { value: true, source: 'probe' as const, at: '2026-07-01T00:00:00.000Z', detail: 'measured before', score: 1 } : null),
      },
    })
    const report = await runProbes('qwen3-14b', { ids: ['json'], deps })

    expect(asked).toEqual([])
    // Nothing is rewritten: the fact already stands, and restamping `at` would
    // make it look freshly measured on every sweep.
    expect(written).toEqual([])
    expect(report.wrote).toBe(0)
    // NOT `skipped`. A skip means no fact exists and an admin should conclude
    // nothing; this means the fact exists and still stands.
    expect(outcomeOf(report, 'json')).toMatchObject({ kind: 'known', at: '2026-07-01T00:00:00.000Z', verdict: { value: true } })
  })

  it('re-measures when asked to, so a re-pointed model id can be re-established', async () => {
    const { deps, written, asked } = harness({
      reply: () => ({ raw: '{"name":"talaria","count":3,"ok":true}', contractHeld: true }),
      over: { measured: () => Promise.resolve({ value: false, source: 'probe' as const, at: '2026-07-01T00:00:00.000Z', detail: 'old', score: 0 }) },
    })
    const report = await runProbes('qwen3-14b', { ids: ['json'], deps, reprobe: true })

    expect(asked.length).toBeGreaterThan(0)
    expect(report.wrote).toBe(1)
    expect(written[0]?.fact).toMatchObject({ value: true, source: 'probe' })
  })

  it('never reuses a DECLARED or LEARNED fact — those are the claims tier 1 exists to verify', async () => {
    // `measured` is documented to return only a `probe` fact, and the default
    // implementation enforces it. This is the assertion that a catalog's
    // marketing copy can never stop us checking the model.
    const { deps, asked } = harness({
      reply: () => ({ raw: '{"name":"talaria","count":3,"ok":true}', contractHeld: true }),
      // The real `measured` filters on source; a dep that returned a declared
      // fact would be a bug in `defaultDeps`, so what is asserted here is that
      // the null it returns for one puts the probe back on the wire.
      over: { measured: () => Promise.resolve(null) },
    })
    await runProbes('qwen3-14b', { ids: ['json'], deps })
    expect(asked.length).toBeGreaterThan(0)
  })

  it('writes NOTHING when a probe errors - an absent fact means unknown, and unknown is safe', async () => {
    // A 401 or a restarting gateway is a fact about the deployment, not about
    // the model. A `json: false` written from one would refuse a working model
    // for good, because probe facts do not expire.
    const { deps, written } = harness({ reply: () => ({ transportError: 'gateway completion 401: bad key' }) })
    const report = await runProbes('qwen3-14b', { ids: ['json', 'instruction-following'], deps })
    expect(written).toEqual([])
    expect(report.wrote).toBe(0)
    expect(outcomeOf(report, 'json')).toMatchObject({ kind: 'errored', reason: 'gateway completion 401: bad key' })
  })

  it('stops calling after a transport failure instead of burning the rest of the trials', async () => {
    const { deps, asked } = harness({ reply: () => ({ transportError: 'connect ECONNREFUSED' }) })
    await runProbes('qwen3-14b', { ids: ['instruction-following'], deps })
    expect(asked).toHaveLength(1)
  })

  it('contains a probe that throws, and still scores the ones around it', async () => {
    const { deps, written } = harness({
      reply: (spec) => {
        if (spec.id.startsWith('json:')) throw new Error('boom')
        return { raw: 'OK' }
      },
      over: {
        ask: (spec) => {
          if (spec.id.startsWith('json:')) return Promise.reject(new Error('boom'))
          const expected = spec.id.endsWith('exactly OK') ? 'OK' : spec.id.endsWith('exactly three words') ? 'red green blue' : '7'
          return Promise.resolve(attempt({ raw: expected }))
        },
      },
    })
    const report = await runProbes('qwen3-14b', { ids: ['json', 'instruction-following'], deps })
    expect(outcomeOf(report, 'json')).toMatchObject({ kind: 'errored', reason: 'boom' })
    expect(outcomeOf(report, 'instruction-following')).toMatchObject({ kind: 'scored' })
    expect(written.map((w) => w.cap)).toEqual(['instruction-following'])
  })

  it('refuses to write when the model resolves to more than one endpoint', async () => {
    // Capability is a property of the ENDPOINT. A bare id served by a pool lands
    // on one member per call, so crediting the result to all of them would give
    // a llama.cpp build the vendor API's tool calling.
    state.endpoints = [endpoint('pl-main'), endpoint('openrouter')]
    const { deps, written } = harness({ reply: () => ({ raw: '{"name":"talaria","count":3,"ok":true}' }) })
    const report = await runProbes('qwen3-14b', { ids: ['json'], deps })
    expect(report.ambiguous).toEqual(['pl-main:qwen3-14b', 'openrouter:qwen3-14b'])
    expect(report.wrote).toBe(0)
    expect(written).toEqual([])
    // The results are still there for a human to read.
    expect(outcomeOf(report, 'json')).toMatchObject({ kind: 'scored' })
  })

  it('writes nothing when no key can be derived at all', async () => {
    state.endpoints = []
    state.personaKeys = []
    const { deps, written } = harness({ reply: () => ({ raw: '{"name":"talaria","count":3,"ok":true}' }) })
    const report = await runProbes('who-is-this', { ids: ['json'], deps })
    expect(report.keys).toEqual([])
    expect(written).toEqual([])
  })

  it('probes a fleet persona through the capability keys of its backing model', async () => {
    state.endpoints = []
    state.personaKeys = ['pl-main:qwen3-14b']
    const { deps, written } = harness({ reply: () => ({ raw: '{"name":"talaria","count":3,"ok":true}' }) })
    const report = await runProbes('assistant-operations', { ids: ['json'], deps })
    expect(report.keys).toEqual(['pl-main:qwen3-14b'])
    expect(written[0]).toMatchObject({ key: 'pl-main:qwen3-14b', cap: 'json' })
  })

  it('does not turn a dropped parameter into a verdict about the model', async () => {
    const { deps, written } = harness({
      reply: () => ({ raw: '{"name":"talaria","count":3,"ok":true}', contractHeld: true, contractDropped: true }),
    })
    await runProbes('qwen3-14b', { ids: ['json'], deps })
    expect(written[0]?.fact).toMatchObject({ value: true, source: 'probe' })
    expect(written[0]?.fact.detail).toContain('dropped response_format')
  })

  // ── The armed tool probes ─────────────────────────────────────────────────
  //
  // `tool-select` is the fact that widens the Inbox command harness from a
  // regex-chosen single action to the item's whole action list (audit 1.8), and
  // until `TransportRequest` grew a slot for tool DEFINITIONS it skipped on
  // every run of every build — so the widening feature could not fire in
  // production and the admin saw a permanent "skipped" on the probe that would
  // arm it. These are the assertions for the armed path, and the strictness is
  // the point: a wrong `true` here hands a 7B model somebody's ticket.

  /** The right answer for each of the four `tool-select` prompts, keyed on the
   *  trial id the probe passes. Written as data so a wrong pick is one edit. */
  const CORRECT: Record<string, string> = {
    'tool-select:weather': 'get_weather',
    'tool-select:email': 'send_email',
    'tool-select:currency': 'convert_currency',
    'tool-select:ticket': 'create_ticket',
  }

  it('scores tool-select from four real calls and records the fact that arms widening', async () => {
    const { deps, written, asked } = harness({
      tools: (spec) => ({ toolCalls: [{ name: CORRECT[spec.id] ?? '?', args: '{}' }] }),
    })
    const report = await runProbes('qwen3-14b', { ids: ['tool-select'], deps })

    expect(asked).toEqual(Object.keys(CORRECT))
    expect(outcomeOf(report, 'tool-select')).toMatchObject({ kind: 'scored', verdict: { value: true, score: 1 } })
    expect(written).toHaveLength(1)
    expect(written[0]).toMatchObject({ cap: 'tool-select', fact: { value: true, source: 'probe', score: 1 } })
  })

  it('REFUSES the fact on 3 of 4 — the fourth pick is an action taken on somebody else’s ticket', async () => {
    const { deps, written } = harness({
      tools: (spec) => ({ toolCalls: [{ name: spec.id === 'tool-select:currency' ? 'send_email' : (CORRECT[spec.id] ?? '?'), args: '{}' }] }),
    })
    const report = await runProbes('qwen3-14b', { ids: ['tool-select'], deps })

    const outcome = outcomeOf(report, 'tool-select')
    expect(outcome).toMatchObject({ kind: 'scored', verdict: { value: false } })
    if (outcome.kind === 'scored') expect(outcome.verdict.score).toBeCloseTo(0.75)
    // Recorded as FALSE rather than left unknown: four conclusive trials with a
    // wrong pick in them is a measurement, and the score says how close it got.
    expect(written[0]).toMatchObject({ cap: 'tool-select', fact: { value: false, source: 'probe' } })
  })

  it('scores `tools` off one offered definition and the arguments that came back', async () => {
    const { deps, written } = harness({ tools: () => ({ toolCalls: [{ name: 'get_weather', args: '{"city":"Lisbon"}' }] }) })
    await runProbes('qwen3-14b', { ids: ['tools'], deps })
    expect(written[0]).toMatchObject({ cap: 'tools', fact: { value: true, source: 'probe' } })

    // Prose instead of a call is the failure this probe exists to catch, and it
    // is what a model with no tool support does.
    const prose = harness({ tools: () => ({ toolCalls: [] }) })
    await runProbes('qwen3-14b', { ids: ['tools'], deps: prose.deps })
    expect(prose.written[0]?.fact).toMatchObject({ value: false })
    expect(prose.written[0]?.fact.detail).toContain('prose')
  })

  it('writes NOTHING when the tool call never completed - a 401 is not a model that cannot call tools', async () => {
    const { deps, written } = harness({ tools: () => ({ transportError: 'gateway completion 401: bad key' }) })
    const report = await runProbes('qwen3-14b', { ids: ['tools', 'tool-select'], deps })
    expect(outcomeOf(report, 'tools').kind).toBe('errored')
    expect(outcomeOf(report, 'tool-select').kind).toBe('errored')
    expect(written).toEqual([])
  })

  it('SKIPS both tool probes on a fleet persona rather than scoring one', async () => {
    // The persona's tool loop runs inside the agent container: tools we offer
    // are neither guaranteed to reach the model nor observable when called, so
    // the transport refuses the call. A skip writes nothing; scoring it would
    // write `tools: false` — permanently — about a model nobody asked.
    const { deps, written, asked } = harness({ over: { offersToolDefinitions: () => Promise.resolve(false) } })
    const report = await runProbes('assistant-operations', { ids: ['tools', 'tool-select'], deps })

    expect(outcomeOf(report, 'tools')).toMatchObject({ kind: 'skipped', reason: expect.stringContaining('fleet persona') })
    expect(outcomeOf(report, 'tool-select')).toMatchObject({ kind: 'skipped', reason: expect.stringContaining('fleet persona') })
    expect(asked).toEqual([])
    expect(written).toEqual([])
  })

  it('gives the STRUCTURAL reason vision cannot run, advertised or not', async () => {
    // It used to skip with "this endpoint does not advertise vision" whenever a
    // catalog said nothing — which was the reason shown for every Claude model,
    // reads as a fact about Claude, and is a fact about a terse catalog. Worse,
    // it hid the real blocker: `Message.content` is a string across the whole
    // tree, so no turn can carry an image part. That is the one thing an admin
    // could act on and it was invisible behind the catalog gate.
    for (const advertises of [false, true]) {
      const { deps } = harness({ over: { advertises: () => Promise.resolve(advertises) } })
      const out = await runProbes('qwen3-14b', { ids: ['vision'], deps })
      expect(outcomeOf(out, 'vision'), String(advertises)).toMatchObject({
        kind: 'skipped',
        reason: expect.stringContaining('image parts'),
      })
    }
  })

  it('MEASURES long context when nothing advertises a window, and says the window was assumed', async () => {
    // Anthropic's /v1/models returns an id and a display name and nothing else,
    // so this skipped on every Claude model — a permanent hole in the matrix for
    // models with some of the largest windows there are. Nothing here may
    // hardcode a provider's window, so the answer is to measure at the probe's
    // own ceiling and SAY that is what happened.
    const { deps } = harness({})
    const out = outcomeOf(await runProbes('qwen3-14b', { ids: ['long-context'], deps }), 'long-context')

    expect(out).toMatchObject({ kind: 'scored' })
    expect(out?.kind === 'scored' && out.verdict.detail).toContain('advertises no window')
  })

  it('still skips a window too small to be called long', async () => {
    // The one long-context skip that is about the MODEL rather than about a
    // catalog: testing 4k proves nothing about long context.
    const { deps: tiny } = harness({ over: { contextWindow: () => Promise.resolve(4_096) } })
    expect(outcomeOf(await runProbes('qwen3-14b', { ids: ['long-context'], deps: tiny }), 'long-context')).toMatchObject({
      kind: 'skipped',
      reason: expect.stringContaining('below the'),
    })
  })

  it('caps a huge advertised window and says in the detail what it actually tested', async () => {
    const { deps, written } = harness({
      over: {
        contextWindow: () => Promise.resolve(1_000_000),
        maxContextTokens: 32_000,
        needleToken: 'GRANITE-FOX-7731',
        ask: () => Promise.resolve(attempt({ raw: 'granite-fox-7731' })),
      },
    })
    await runProbes('qwen3-14b', { ids: ['long-context'], deps })
    expect(written[0]?.fact).toMatchObject({ value: true, source: 'probe' })
    // 80% of the 32k cap, not 90% of the million it advertises.
    expect(written[0]?.fact.detail).toContain('25,600')
  })

  it('verifies a search citation by fetching the page, and calls an unreadable page inconclusive', async () => {
    const reply = JSON.stringify({
      date: '2026-08-06',
      url: 'https://news.example-press.org/ferry',
      quote: 'The council approved the new ferry timetable on Tuesday evening after a long debate.',
    })
    const good = harness({
      reply: () => ({ raw: reply }),
      over: { fetchText: () => Promise.resolve('<p>The council approved the new ferry timetable on Tuesday evening after a long debate.</p>') },
    })
    await runProbes('qwen3-14b', { ids: ['search'], deps: good.deps })
    expect(good.written[0]?.cap).toBe('search')
    expect(good.written[0]?.fact).toMatchObject({ value: true, source: 'probe' })

    const blocked = harness({ reply: () => ({ raw: reply }), over: { fetchText: () => Promise.resolve(null) } })
    const report = await runProbes('qwen3-14b', { ids: ['search'], deps: blocked.deps })
    expect(blocked.written).toEqual([])
    expect(outcomeOf(report, 'search').kind).toBe('skipped')
  })

  it('records search: false when the model cannot name today on either trial', async () => {
    const stale = JSON.stringify({ date: '2024-06-01', url: 'https://example.com/x', quote: 'x'.repeat(50) })
    const { deps, written } = harness({ reply: () => ({ raw: stale }) })
    await runProbes('qwen3-14b', { ids: ['search'], deps })
    expect(written[0]).toMatchObject({ cap: 'search' })
    expect(written[0]?.fact).toMatchObject({ value: false, source: 'probe' })
  })

  it('is stable: the same recorded replies score identically twice', async () => {
    const replies = (spec: AskSpec): Partial<Attempt> => {
      if (spec.id.startsWith('json:')) return { raw: '{"name":"talaria","count":3,"ok":true}' }
      if (spec.id.startsWith('code:slugify')) return { raw: GOOD_SLUGIFY }
      if (spec.id.startsWith('code:')) return { raw: 'function mergeRanges(r) { return r }' }
      return { raw: 'OK' }
    }
    const ids: ProbeId[] = ['json', 'code', 'instruction-following']
    const first = harness({ reply: replies })
    const second = harness({ reply: replies })
    await runProbes('qwen3-14b', { ids, deps: first.deps })
    await runProbes('qwen3-14b', { ids, deps: second.deps })
    expect(second.written).toEqual(first.written)
    expect(first.written.map((w) => [w.cap, w.fact.value, w.fact.score])).toEqual([
      ['json', true, 1],
      ['instruction-following', false, 1 / 3],
      ['code', false, 0.5],
    ])
  })

  it('reads latency off the existing gateway pulse ring rather than timing its own calls', async () => {
    const { deps } = harness({})
    const report = await runProbes('qwen3-14b', { ids: ['json'], deps })
    expect(report.latency).toMatchObject({ requests: 12, errors: 1, p50: 140, p95: 620 })
  })
})

// ── The estimate ─────────────────────────────────────────────────────────────

describe('estimateProbes', () => {
  it('reports calls and tokens per probe before anything is spent', async () => {
    const { deps } = harness({})
    const est = await estimateProbes('qwen3-14b', { ids: ['json', 'instruction-following'], deps })
    expect(est.calls).toBe(6)
    expect(est.promptTokens).toBeGreaterThan(0)
    expect(est.rows.map((r) => r.id)).toEqual(['json', 'instruction-following'])
    expect(est.usd).toBeNull()
  })

  it('prices the run when the endpoint has a price, using $/MTok', async () => {
    const { deps } = harness({ over: { price: () => Promise.resolve({ in: 1, out: 2 }) } })
    const est = await estimateProbes('qwen3-14b', { ids: ['json'], deps })
    expect(est.usd).toBeCloseTo((est.promptTokens * 1 + est.completionTokens * 2) / 1e6)
  })

  it('sizes long-context from the capped window rather than from a fixture', async () => {
    const { deps } = harness({ over: { contextWindow: () => Promise.resolve(1_000_000), maxContextTokens: DEFAULT_MAX_CONTEXT_TOKENS } })
    const est = await estimateProbes('qwen3-14b', { ids: ['long-context'], deps })
    expect(est.rows[0]?.promptTokens).toBe(Math.floor(DEFAULT_MAX_CONTEXT_TOKENS * 0.8))
    expect(est.calls).toBe(2)
  })

  it('bills long-context at the probe ceiling when no window is advertised, because it now runs', async () => {
    // The estimate reads the same edges the run reads. It used to bill zero here
    // on the grounds that the probe would skip; the probe no longer skips, and an
    // estimate that still said zero would understate a real 25,600-token pair of
    // calls.
    const { deps } = harness({})
    const est = await estimateProbes('qwen3-14b', { ids: ['long-context'], deps })
    expect(est.calls).toBe(2)
    expect(est.promptTokens).toBeGreaterThan(20_000)
  })

  it('CHARGES NOTHING FOR A PROBE THAT WILL SKIP — a fleet candidate and an endpoint with no vision', async () => {
    // The estimate is the sentence in front of a button that spends someone
    // else's inference budget, and billing six calls for three probes that
    // cannot run overstates a probes-only run by a fifth — exactly the kind of
    // number that makes an admin stop trusting the rest of the page.
    const { deps } = harness({ over: { offersToolDefinitions: () => Promise.resolve(false) } })
    const est = await estimateProbes('assistant-operations', { ids: ['tools', 'tool-select', 'vision'], deps })
    expect(est.calls).toBe(0)
    expect(est.usd).toBeNull()
  })

  it('BILLS THE TOOL PROBES NOW THEY ARE ARMED, because that is the number an admin decides to spend', async () => {
    // The other half of the same honesty: a run that WILL make five tool calls
    // must say five. `askWithImages` is still shut, so vision alone stays free.
    const { deps } = harness({ over: { advertises: () => Promise.resolve(true) } })
    const est = await estimateProbes('qwen3-14b', { ids: ['tools', 'tool-select', 'vision'], deps })
    expect(est.calls).toBe(5)
    expect(est.rows.find((r) => r.id === 'tools')?.calls).toBe(1)
    expect(est.rows.find((r) => r.id === 'tool-select')?.calls).toBe(4)
    expect(est.rows.find((r) => r.id === 'vision')?.calls).toBe(0)
  })

  it('MATCHES THE CALLS THE RUN ACTUALLY MAKES, over every probe that can run here', async () => {
    // The estimate and the run read the SAME edges, so this is a real invariant
    // rather than two constants agreeing. Counted against the asks both the
    // text and the tool channels record.
    const { deps, asked } = harness({
      over: { contextWindow: () => Promise.resolve(32_000), advertises: () => Promise.resolve(true) },
      reply: () => ({ raw: 'OK' }),
      tools: () => ({ toolCalls: [{ name: 'get_weather', args: '{"city":"Lisbon"}' }] }),
    })
    const est = await estimateProbes('qwen3-14b', { deps })
    await runProbes('qwen3-14b', { deps })
    expect(asked).toHaveLength(est.calls)
  })
})

// ── The production `ask`: runHarness with the candidate pinned ───────────────

describe('runnerAsk', () => {
  const OBJECT = z.object({ name: z.string(), count: z.number(), ok: z.boolean() })
  const GOOD = '{"name":"talaria","count":3,"ok":true}'
  const reply = (over: Partial<TransportReply> = {}): TransportReply => ({
    kind: 'gateway',
    text: '',
    toolNames: [],
    usage: null,
    contractDropped: false,
    ...over,
  })
  const prompt = [{ role: 'user' as const, content: 'return the object' }]

  it('pins the candidate and asks for JSON at the protocol level', async () => {
    const seen: TransportRequest[] = []
    const ask = runnerAsk('vendor/frontier-1', (req) => {
      seen.push(req)
      return Promise.resolve(reply({ text: GOOD }))
    })
    const a = await ask({ id: 'json:trivial', messages: prompt, schema: OBJECT })
    expect(seen[0]?.model).toBe('vendor/frontier-1')
    expect(seen[0]?.jsonMode).toBe(true)
    expect(a).toMatchObject({ contractHeld: true, jsonRequested: true, contractDropped: false, transportError: null })
    expect(a.raw).toContain('talaria')
  })

  it('measures the FIRST attempt: no repair turn, so the score is the contract rate the fitness page means', async () => {
    let calls = 0
    const ask = runnerAsk('vendor/frontier-1', () => {
      calls++
      return Promise.resolve(reply({ text: 'Sure! Here is the object you asked for.' }))
    })
    const a = await ask({ id: 'json:trivial', messages: prompt, schema: OBJECT })
    expect(calls).toBe(1)
    expect(a.contractHeld).toBe(false)
  })

  it('carries the gateway contract drop through, which is the whole audit-1.2 signal', async () => {
    const ask = runnerAsk('vendor/frontier-1', () => Promise.resolve(reply({ text: GOOD, contractDropped: true })))
    expect(await ask({ id: 'json:trivial', messages: prompt, schema: OBJECT })).toMatchObject({ contractDropped: true, contractHeld: true })
  })

  it('reports a transport throw as a transport error, not as a model that answered badly', async () => {
    const ask = runnerAsk('vendor/frontier-1', () => Promise.reject(new Error('gateway completion 401: bad key')))
    const a = await ask({ id: 'json:trivial', messages: prompt, schema: OBJECT })
    expect(a.transportError).toBe('gateway completion 401: bad key')
    expect(a.contractHeld).toBe(false)
  })

  it('hands a text probe the reply verbatim, because "exactly OK" means exactly', async () => {
    const ask = runnerAsk('vendor/frontier-1', () => Promise.resolve(reply({ text: '  OK\n' })))
    const a = await ask({ id: 'instruction:exactly OK', messages: prompt })
    expect(a.raw).toBe('  OK\n')
    expect(a.jsonRequested).toBe(false)
  })

  // ── The tool ask: the same runner, with definitions on the request ─────────

  const TOOL = { name: 'get_weather', description: 'Current weather for a city.', parameters: { type: 'object', properties: { city: { type: 'string' } } } }

  it('puts the definitions on the request and reports the calls back', async () => {
    const seen: TransportRequest[] = []
    const ask = runnerToolAsk('vendor/frontier-1', (req) => {
      seen.push(req)
      return Promise.resolve(reply({ toolCalls: [{ name: 'get_weather', args: '{"city":"Lisbon"}' }], toolNames: ['get_weather'] }))
    })
    const a = await ask({ id: 'tools', messages: prompt, tools: [TOOL] })

    expect(seen[0]?.model).toBe('vendor/frontier-1')
    expect(seen[0]?.toolDefs).toEqual([TOOL])
    // A tool-calling turn usually returns EMPTY content, which every text
    // contract in the tree reads as a failure — so the probe grades the calls,
    // not the value, and a failed contract here is not a failed trial.
    expect(seen[0]?.jsonMode).toBe(false)
    expect(a).toEqual({ toolCalls: [{ name: 'get_weather', args: '{"city":"Lisbon"}' }], transportError: null })
  })

  it('treats a transport that reports NO tool-call channel as an error, not as a model that called nothing', async () => {
    // ABSENT IS NOT EMPTY. `pickTransport` cannot produce this — the fleet path
    // refuses a request carrying definitions — but a bespoke transport could,
    // and reading undefined as "called nothing" would write `tools: false`
    // forever about a model that was never offered a tool.
    const ask = runnerToolAsk('vendor/frontier-1', () => Promise.resolve(reply({ text: 'sure, it is sunny' })))
    const a = await ask({ id: 'tools', messages: prompt, tools: [TOOL] })
    expect(a.toolCalls).toEqual([])
    expect(a.transportError).toContain('without reporting any tool calls')
  })

  it('reports a refusal from the transport as a transport error, which voids the probe', async () => {
    const ask = runnerToolAsk('assistant-operations', () => Promise.reject(new Error('its tool loop runs inside the agent container')))
    const a = await ask({ id: 'tools', messages: prompt, tools: [TOOL] })
    expect(a.transportError).toContain('tool loop runs inside the agent')
  })
})

// ── The real deps, in the shape the server will build them ───────────────────

describe('defaultDeps', () => {
  it('reads the SMALLEST advertised window in the pool, because a claim has to hold for the worst member', async () => {
    state.endpoints = [endpoint('pl-main', { contextLength: 128_000 }), endpoint('local', { contextLength: 32_768 })]
    expect(await defaultDeps('qwen3-14b').contextWindow()).toBe(32_768)
  })

  it('reads the DEAREST price in the pool, so the estimate cannot be exceeded by the endpoint that answers', async () => {
    state.endpoints = [
      endpoint('cheap', { modelPrices: { 'qwen3-14b': { in: 0.1, out: 0.2 } } }),
      endpoint('dear', { autoPrices: { 'qwen3-14b': { in: 3, out: 9 } } }),
    ]
    expect(await defaultDeps('qwen3-14b').price()).toEqual({ in: 3, out: 9 })
  })

  it('answers null for both when nothing is known, rather than guessing', async () => {
    state.endpoints = [endpoint('pl-main')]
    const deps = defaultDeps('qwen3-14b')
    expect(await deps.contextWindow()).toBeNull()
    expect(await deps.price()).toBeNull()
  })

  it('opens every ask channel, so no capability goes unmeasured for want of a seam', () => {
    // Both were null once and each left a permanent hole in the matrix.
    // `askWithTools` carries real definitions through `TransportRequest.toolDefs`
    // and reads back what was called. `askWithImages` builds the multimodal body
    // itself and hands it to the same gateway plumbing `completeViaGateway` uses
    // — which measures the MODEL without widening `Message.content` tree-wide,
    // the change that argument was really about.
    const deps = defaultDeps('qwen3-14b')
    expect(typeof deps.askWithTools).toBe('function')
    expect(typeof deps.offersToolDefinitions).toBe('function')
    expect(typeof deps.askWithImages).toBe('function')
  })

  it('caps context spend by default - a 200k window probed at 90% is dollars, not cents', () => {
    expect(defaultDeps('qwen3-14b').maxContextTokens).toBe(DEFAULT_MAX_CONTEXT_TOKENS)
    expect(MIN_LONG_CONTEXT_TOKENS).toBeLessThan(DEFAULT_MAX_CONTEXT_TOKENS)
  })
})

describe('the wall clock on a probe', () => {
  it('gives up on a transport that never settles, and writes nothing', async () => {
    // Tier 2 races every case; tier 1 raced nothing. A provider that accepted the
    // connection and went away left `runProbes` awaiting a promise that never
    // settled — holding a run slot forever, unreachable by Stop (honored only
    // between tiers). With eight candidates able to run at once, a few hung calls
    // take slots permanently.
    const { deps, written } = harness({ over: { ask: () => new Promise(() => {}) } })
    const report = await runProbes('qwen3-14b', { ids: ['instruction-following'], deps, timeoutMs: 30 })

    const outcome = outcomeOf(report, 'instruction-following')
    expect(outcome.kind).toBe('errored')
    // A timeout measured NOTHING about the model, so by rule 2 no fact is stored.
    expect(written).toHaveLength(0)
  })
})

describe('an endpoint that refuses the image itself', () => {
  it('records the model as unable to take images, not the deployment as broken', async () => {
    // OpenRouter answers a text-only model with `404 No endpoints found that
    // support image input`. That is not a broken gateway — it is the deployment
    // saying plainly what this model can be sent, which is exactly what a
    // capability key (`endpoint:model`) addresses. It used to land in `errored`,
    // which writes nothing and reads to an admin as "something is wrong".
    const { deps, written } = harness({
      over: {
        askWithImages: () =>
          Promise.resolve(attempt({ transportError: 'gateway completion 404: {"error":{"message":"No endpoints found that support image input"}}' })),
      },
    })
    const report = await runProbes('qwen3-14b', { ids: ['vision'], deps })

    expect(outcomeOf(report, 'vision')).toMatchObject({ kind: 'scored' })
    expect(written[0]?.cap).toBe('vision')
    expect(written[0]?.fact).toMatchObject({ value: false })
  })

  it('leaves an ordinary outage as an error, because nothing was measured', async () => {
    const { deps, written } = harness({
      over: { askWithImages: () => Promise.resolve(attempt({ transportError: 'gateway completion 503: upstream restarting' })) },
    })
    const report = await runProbes('qwen3-14b', { ids: ['vision'], deps })

    expect(outcomeOf(report, 'vision').kind).toBe('errored')
    expect(written).toHaveLength(0)
  })
})
