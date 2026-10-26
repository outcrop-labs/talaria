import { describe, expect, it } from 'vitest'
import { z } from 'zod'

// THE DOCUMENTED SNIPPETS, COMPILED AND RUN.
//
// `docs/SDK.md` described a React SDK for months after the tree stopped having
// one, and nothing failed — a document is not a build target. The activity
// harness surface and the bridge pattern are extension points with the same
// exposure, and the cost of a wrong line here lands on a third-party app author
// who cannot tell whether the mistake is theirs.
//
// So the examples in `docs/sdk/harnesses.md` — the triage harness and the
// bridge that runs it live with an app-side fallback — are transcribed here
// VERBATIM, imported from `@talaria/sdk/server` exactly as the doc says to, and
// run through the real runner against recorded replies. A rename anywhere in
// the contract now breaks the typecheck of the documentation itself.
import {
  defineHarness,
  belowAnswerFloor,
  NO_TOOLS,
  resolveHarnessModel,
  runHarness as runHarnessFromSdk,
  type EvalCase,
  type HarnessDefinition,
  type RunContext,
  type ToolDefinition,
  type ToolCall,
} from './server'
import { runHarnessOffline } from './testing'
import { runHarness } from '@/server/harness/run'
import { defineHarness as platformDefineHarness } from '@/server/harness/define'

const TRIAGE = z.object({ severity: z.enum(['low', 'medium', 'high']), reason: z.string() })
type Triage = z.infer<typeof TRIAGE>
interface TriageInput {
  subject: string
  body: string
}

const triage = defineHarness<TriageInput, Triage>({
  id: 'support:triage',
  label: 'Support triage',
  job: 'Grades an inbound support message so the queue can order itself.',
  requires: ['json', 'instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Runs on any model that can return JSON; a weak one grades more coarsely and the queue stays usable.',
  },
  model: {},
  render: (input) => [
    { role: 'system', content: 'Grade this support message. Reply with severity and a one-line reason.' },
    { role: 'user', content: `${input.subject}\n\n${input.body}` },
  ],
  output: { kind: 'json', schema: TRIAGE },
  onFailure: 'null',
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  evals: [
    {
      name: 'an outage report grades high',
      input: { subject: 'Checkout is down', body: 'Nobody can pay since 09:00. Every card is declined.' },
      check: (v) => (v.severity === 'high' ? null : `graded "${v.severity}" — a total checkout outage is high`),
    },
  ],
})

const GUARD_OFF = { mode: 'off' as const, checks: {}, minConfidence: 1, policedHosts: [], coach: false }

/** The runner with everything outside the contract scripted — the same seam
 *  `recorded.ts` uses, spelled locally so this file imports only what the doc
 *  tells an app author to import plus the runner that executes it. */
const answer = (text: string): Parameters<typeof runHarness>[2] => ({
  caller: 'test:sdk-doc',
  model: 'qwen3-14b',
  deps: {
    routing: async (m: string) => ({ endpoints: ['pl-main'], upstreamModel: m }),
    transport: async () => ({ kind: 'gateway' as const, text, toolNames: [], usage: null, contractDropped: false }),
    capabilities: async () => ({}),
    missingCapabilities: async () => [],
    guardConfig: async () => GUARD_OFF,
    guardText: async () => [],
    recordFindings: async () => {},
    recordRun: async () => {},
  },
})

/** The same runner, offline, on a model whose record says it cannot produce
 *  JSON — `source` is who said so. `requests` counts the model calls, because
 *  the claim under test is that a refusal never reaches the model. */
const unableToJson = (source: 'probe' | 'declared' | 'learned', input: TriageInput) =>
  runHarnessOffline(triage, input, {
    model: 'qwen3-14b',
    replies: ['{"severity":"high","reason":"total checkout outage"}'],
    capabilities: { json: { value: false, source } },
  })

describe("SDK.md's activity-harness example", () => {
  it('runs through the real runner and satisfies its own fixture', async () => {
    const fixture = triage.evals?.[0]
    expect(fixture).toBeDefined()
    const result = await runHarness(triage, fixture!.input, answer('{"severity":"high","reason":"total checkout outage"}'))
    expect(result.schemaValid).toBe(true)
    expect(fixture!.check(result.value as Triage, NO_TOOLS)).toBeNull()
  })

  it('applies the failure policy it declares — a model that answers in prose returns null, it does not throw', async () => {
    // `onFailure: 'null'` in the doc's snippet. An app author copying it is
    // entitled to the behavior the doc's comment claims.
    const result = await runHarness(triage, { subject: 'hi', body: 'hello' }, answer('I would say this is probably medium?'))
    expect(result.value).toBeNull()
    expect(result.schemaValid).toBe(false)
  })
})

describe("docs/sdk/harnesses.md's bridge example", () => {
  // The bridge's core, transcribed: probe, run, label. The fetch plumbing
  // around it (path check, request.json, the host-bound `ctx.harness`) is not
  // executable without a request; the run here is the doc's testing road
  // (`runHarnessOffline`), and the label expressions are the doc's, verbatim.
  it('labels the run model-vs-fallback exactly as the doc does', async () => {
    const good = await runHarnessOffline(
      triage,
      { subject: 'Checkout is down', body: 'Every card declined since 09:00.' },
      { model: 'qwen3-14b', replies: ['{"severity":"high","reason":"total checkout outage"}'] },
    )
    expect(good.value?.severity).toBe('high')
    expect(good.value ? `model:${good.model}` : 'fallback').toBe('model:qwen3-14b')

    const prose = await runHarnessOffline(triage, { subject: 'hi', body: 'hello' }, { model: 'qwen3-14b', replies: ['I would say this is probably medium?'] })
    expect(prose.value).toBeNull()
    expect(prose.value ? `model:${prose.model}` : 'fallback').toBe('fallback')
  })

  it("runHarnessOffline repairs once on a malformed reply, as production does, and shows the prompts", async () => {
    const run = await runHarnessOffline(triage, { subject: 'Checkout is down', body: 'Every card declined.' }, {
      replies: ['not json', '{"severity":"high","reason":"outage"}'],
    })
    expect(run.repairs).toBe(1)
    expect(run.value?.severity).toBe('high')
    expect(run.requests).toHaveLength(2)
    expect(run.requests[0]!.messages.at(-1)?.content).toContain('Checkout is down')
  })

  // The doc's probe (`resolveHarnessModel(triage.model)`) reads the org's model
  // settings, so it is exercised only as a shape in the exports test below —
  // here it would need a database. The bridge's null branch is its behavior.
})

describe("the SDK's defineHarness is the platform's, JSON floor included", () => {
  // THE GAP THIS CLOSES. The SDK used to return the definition untouched, so an
  // app's JSON harness ran on a model a probe had measured unable to produce
  // JSON — the platform refuses there — and the app handed the prose to the
  // parser and recorded the wreckage as the model's failure.
  it('is the same function the platform harnesses use', () => {
    expect(defineHarness).toBe(platformDefineHarness)
  })

  it("derives the doc example's floor: json, refusing, the author's note kept", () => {
    expect(triage.floor.capabilities).toEqual(['json'])
    expect(triage.floor.refuseBelow).toBe(true)
    expect(triage.floor.note).toBe('Runs on any model that can return JSON; a weak one grades more coarsely and the queue stays usable.')
  })

  it('leaves a text harness exactly as declared', () => {
    const texty = defineHarness<{ q: string }, string>({
      id: 'sdk:texty-floor',
      label: 'Texty',
      job: 'echoes a question back as a sentence',
      requires: [],
      floor: { capabilities: [], refuseBelow: false, note: 'Runs on any model.' },
      model: {},
      render: (input) => [{ role: 'user', content: input.q }],
      output: { kind: 'text' },
      onFailure: 'null',
    })
    expect(texty.floor).toEqual({ capabilities: [], refuseBelow: false, note: 'Runs on any model.' })
  })

  it('refuses a model a probe measured unable to produce JSON, without calling it', async () => {
    const run = await unableToJson('probe', { subject: 'Checkout is down', body: 'Every card declined.' })
    expect(run.requests).toHaveLength(0)
    expect(run.refused).toBe(true)
    expect(run.value).toBeNull()
    expect(run.error).toContain('json')
    expect(run.error).toContain(triage.floor.note)
    // The bridge's own label still reads as the doc's: no value is a fallback.
    expect(run.value ? `model:${run.model}` : 'fallback').toBe('fallback')
  })

  it('refuses on an admin-declared fact too', async () => {
    const run = await unableToJson('declared', { subject: 'a', body: 'b' })
    expect(run.requests).toHaveLength(0)
    expect(run.refused).toBe(true)
  })

  it('still runs on a fact the gateway only learned from one 400', async () => {
    const run = await unableToJson('learned', { subject: 'Checkout is down', body: 'Every card declined.' })
    expect(run.requests).toHaveLength(1)
    expect(run.refused).toBe(false)
    expect(run.value?.severity).toBe('high')
  })
})

describe('the rest of the exported surface, as the docs list it', () => {
  it('exports EvalCase for a fixture written apart from its harness', () => {
    const apart: EvalCase<TriageInput, Triage> = {
      name: 'apart from the harness',
      input: { subject: 'a', body: 'b' },
      check: (v) => (v.reason.length > 3 ? null : 'say why the message got that severity'),
    }
    expect(apart.check({ severity: 'low', reason: 'ok' }, NO_TOOLS)).toBe('say why the message got that severity')
  })

  it('exports belowAnswerFloor, the floor a one-sided text fixture needs', () => {
    expect(belowAnswerFloor('yes', { minChars: 40, mentions: ['checkout', 'payment'] })).not.toBeNull()
    expect(
      belowAnswerFloor('The checkout outage stopped every card payment from about nine this morning.', {
        minChars: 40,
        mentions: ['checkout', 'payment'],
      }),
    ).toBeNull()
  })

  it('exports ToolDefinition and ToolCall, so `toolDefs` is nameable outside a literal', () => {
    // The gap this closes: `toolDefs` is a field on the exported definition, so
    // an author who factors their tools into a const had no type to annotate it
    // with and inferred `parameters: { type: string }` instead.
    const tools: ToolDefinition[] = [
      { name: 'get_weather', description: 'Current weather for a city.', parameters: { type: 'object', properties: { city: { type: 'string' } } } },
    ]
    const call: ToolCall = { name: 'get_weather', args: '{"city":"Lisbon"}' }
    expect(tools[0]?.name).toBe(call.name)
  })

  it("the free runHarness's context has no deps: an app cannot swap the guard, the transport or the recorders", () => {
    // @ts-expect-error — `deps` is not part of the SDK's RunContext.
    const ctx: RunContext = { caller: 'app:x', deps: {} }
    expect(ctx.caller).toBe('app:x')
  })

  it('exports runHarness and resolveHarnessModel, so an app bridge can run its own harnesses', () => {
    // The gap this closes: a bridge (live-if-routable with an app-side
    // fallback) is the app's OWN server code invoking its harnesses — today the
    // only thing that runs one at all — and `@/server/harness/*` specifiers
    // cannot resolve from apps/, where only the SDK ids reach. The bridge needs
    // the same runner and the free probe, or it cannot exist outside the ui root.
    // `RunContext` rides along: a bridge building its caller/signal apart from
    // the call has no type to annotate the context with otherwise.
    const ctx: RunContext = { caller: 'app:leadworks:intake' }
    expect(typeof runHarnessFromSdk).toBe('function')
    expect(typeof resolveHarnessModel).toBe('function')
    expect(ctx.caller).toBe('app:leadworks:intake')
  })

  it('HarnessDefinition means the ACTIVITY contract and takes two type arguments', () => {
    const texty: HarnessDefinition<{ q: string }, string> = {
      id: 'sdk:texty',
      label: 'Texty',
      job: 'echoes a question back as a sentence',
      requires: [],
      floor: { capabilities: [], refuseBelow: false, note: 'Runs on any model.' },
      model: {},
      render: (input) => [{ role: 'user', content: input.q }],
      output: { kind: 'text' },
      onFailure: 'null',
    }
    expect(texty.output.kind).toBe('text')
  })
})

describe('a tool loop, offline', () => {
  const COUNT = z.object({ orders: z.number() })
  const counter = defineHarness<{ email: string }, z.infer<typeof COUNT>>({
    id: 'sdk:counter',
    label: 'Counter',
    job: 'Counts a customer’s orders.',
    requires: ['json', 'tools'],
    floor: { capabilities: [], refuseBelow: false, note: 'Any model that calls tools.' },
    model: {},
    render: (i) => [{ role: 'user', content: `count orders for ${i.email}` }],
    output: { kind: 'json', schema: COUNT },
    onFailure: 'null',
    loop: {
      tools: [
        { name: 'find_customer', description: 'Find a customer.', parameters: { type: 'object' }, handler: (args) => ({ id: 'c-7', email: args.email }) },
        { name: 'recent_orders', description: 'Orders.', parameters: { type: 'object' }, mcp: 'orders_recent', simulate: () => ({ count: 3 }) },
      ],
    },
  })

  it('runs handlers for real and answers mcp tools through simulate, with every call on the result', async () => {
    const run = await runHarnessOffline(counter, { email: 'ada@example.test' }, {
      replies: [
        { toolCalls: [{ name: 'find_customer', args: { email: 'ada@example.test' } }] },
        { toolCalls: [{ name: 'recent_orders', args: { customer: 'c-7' } }] },
        '{"orders":3}',
      ],
    })
    expect(run.value).toEqual({ orders: 3 })
    expect(run.calls?.map((c) => [c.tool, c.result])).toEqual([
      ['find_customer', { id: 'c-7', email: 'ada@example.test' }],
      ['recent_orders', { count: 3 }],
    ])
    expect(run.requests).toHaveLength(3)
  })

  it('simulate: true answers every tool through simulate, as the fitness sweep does', async () => {
    const run = await runHarnessOffline(counter, { email: 'e' }, { simulate: true, replies: [{ toolCalls: [{ name: 'find_customer' }] }, '{"orders":0}'] })
    expect(run.calls?.[0]?.error).toBe('tool "find_customer" has no simulate, so a dry run cannot answer it')
  })
})
