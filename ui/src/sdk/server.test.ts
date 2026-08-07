import { describe, expect, it } from 'vitest'
import { z } from 'zod'

// THE DOCUMENTED SNIPPET, COMPILED AND RUN.
//
// `docs/SDK.md` described a React SDK for months after the tree stopped having
// one, and nothing failed — a document is not a build target. The activity
// harness surface is a brand new extension point with the same exposure, and
// the cost of a wrong line here lands on a third-party app author who cannot
// tell whether the mistake is theirs.
//
// So the example in SDK.md's "Activity harnesses" section is transcribed here
// VERBATIM, imported from `@talaria/sdk/server` exactly as the doc says to, and
// run through the real runner against a recorded reply. A rename anywhere in the
// contract now breaks the typecheck of the documentation itself.
import { defineHarness, defineWorkbenchHarness, belowAnswerFloor, NO_TOOLS, type EvalCase, type HarnessDefinition, type ToolDefinition, type ToolCall } from './server'
import { runHarness } from '@/server/harness/run'

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
    note: 'Runs on any model; a weak one grades more coarsely and the queue stays usable.',
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

describe('the rest of the exported surface, as SDK.md lists it', () => {
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

  it('keeps the deprecated workbench spelling building, because renaming an extension point is a break', () => {
    const wb = defineWorkbenchHarness({
      slug: 'aider',
      label: 'Aider',
      auth: 'gateway',
      invoke: 'aider --model <model> --message "<task>"',
      guide: 'Aider works in git-aware sessions.',
    })
    // The overload, not a second function: same identity, different contract.
    expect(defineHarness(wb)).toBe(wb)
  })
})
