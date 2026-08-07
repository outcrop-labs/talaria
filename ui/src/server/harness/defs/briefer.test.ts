import { describe, expect, it } from 'vitest'
import { briefingChatHarness, briefingHarness, type BriefingInput, type BriefingScope } from '@/server/harness/defs/briefer'
import { runHarness, type HarnessDeps, type TransportRequest } from '@/server/harness/run'

const GOOD_BRIEF = [
  '**Blocked** — "Ledger migration" on Platform has been stuck for a day.',
  '**Mention** — Priya asked about the rollback window.',
  '**Unread** — 3 messages in #platform.',
].join('\n')

/** Every edge stubbed. `personaKeys` is deliberately not overridden: `routing`
 *  answers with an endpoint, so the runner never falls through to it. */
function deps(reply: string, capable = false): { deps: Partial<HarnessDeps>; requests: TransportRequest[] } {
  const requests: TransportRequest[] = []
  return {
    requests,
    deps: {
      routing: async (model: string) => ({ endpoints: ['spark'], upstreamModel: model }),
      missingCapabilities: async () => [],
      // Only a PROBE fact widens (run.ts step 3), so the shape matters here.
      capabilities: async () => (capable ? { 'instruction-following': { value: true, source: 'probe' as const } } : {}),
      transport: async (req: TransportRequest) => {
        requests.push(req)
        return { kind: 'fleet' as const, text: reply, toolNames: [], usage: null, contractDropped: false }
      },
      guardConfig: async () => ({ mode: 'observe' as const, checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }),
      guardText: async () => [],
      recordFindings: async () => {},
      recordRun: async () => {},
      now: () => 0,
    },
  }
}

const LINES = ['ticket blocked: "Ledger migration" on Platform', 'notification (mention): Priya asked about the rollback window', '3 unread in #platform']

const brief = (scope: BriefingScope, lines: string[] = LINES, empty = false): BriefingInput => ({ scope, lines, empty })

describe('the briefing definition', () => {
  it('is the one harness with no model chain at all', () => {
    // LOCKED, so that adding a fallback is a decision rather than a diff.
    // `PLATFORM_AGENTS.briefer` is the single `assignable: false` entry — the
    // briefer reads the owner's private attention state, so it is always their
    // own assistant and there is no acceptable second choice. Both halves take
    // the model as an explicit `RunContext.model`.
    for (const h of [briefingHarness, briefingChatHarness]) {
      expect(h.model.chain).toEqual([])
      expect(h.model.pin).toBeUndefined()
      expect(h.model.role).toBeUndefined()
    }
  })

  it('runs end to end without asking for protocol JSON or setting a temperature', async () => {
    const { deps: d, requests } = deps(GOOD_BRIEF)
    const res = await runHarness(briefingHarness, brief('inbox'), { caller: 'test', model: 'penny-assistant', deps: d })
    expect(res.value).toBe(GOOD_BRIEF)
    expect(res.schemaValid).toBe(true)
    expect(requests[0]?.jsonMode).toBe(false)
    // The persona's own default temperature, exactly as this call has always
    // been made — a briefing is prose, not a gate.
    expect(requests[0]?.temperature).toBeUndefined()
  })

  it('sends the scope’s own prompt and the attention lines, and nothing else', async () => {
    const { deps: d, requests } = deps(GOOD_BRIEF)
    await runHarness(briefingHarness, brief('comms'), { caller: 'test', model: 'penny-assistant', deps: d })
    const sent = requests[0]?.messages[0]?.content ?? ''
    expect(sent).toContain('[Automated comms briefing — no human sent this.]')
    expect(sent).toContain('unread channels, relays, DMs, and mentions')
    expect(sent).toContain('- 3 unread in #platform')
    expect(sent).toContain('at most 5 bullets')
  })

  it('asks the empty state for one short line instead of the bullet rules', async () => {
    const { deps: d, requests } = deps('You are all clear.')
    await runHarness(briefingHarness, brief('boards', [], true), { caller: 'test', model: 'penny-assistant', deps: d })
    const sent = requests[0]?.messages[0]?.content ?? ''
    expect(sent).toContain('queues are clear. One short line.')
    expect(sent).not.toContain('at most 5 bullets')
  })

  it('only asks for synthesis from a model proven to follow instructions', async () => {
    const narrow = deps(GOOD_BRIEF, false)
    const wide = deps(GOOD_BRIEF, true)
    const input = brief('inbox')
    const a = await runHarness(briefingHarness, input, { caller: 'test', model: 'penny-assistant', deps: narrow.deps })
    const b = await runHarness(briefingHarness, input, { caller: 'test', model: 'penny-assistant', deps: wide.deps })
    expect(a.widened).toBe(false)
    expect(b.widened).toBe(true)
    expect(narrow.requests[0]?.messages[0]?.content).not.toContain('what this adds up to')
    expect(wide.requests[0]?.messages[0]?.content).toContain('what this adds up to')
    // Widening adds a lead LINE, never a sixth bullet: the cap survives it, and
    // that is what makes the five-bullet fixture valid under both surfaces.
    expect(wide.requests[0]?.messages[0]?.content).toContain('at most 5 bullets')
  })

  it('keeps the previous summary when the model answers with nothing', async () => {
    const { deps: d } = deps('   \n  ')
    const res = await runHarness(briefingHarness, brief('inbox'), { caller: 'test', model: 'penny-assistant', deps: d })
    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
  })

  it('does not police the reporting verbs a briefing is made of', () => {
    // `zero_tool_claim` fires on "Priya sent 3 messages" and "2 tickets moved
    // to review", which is a briefing doing its job. `fabricated_outage` fires
    // on "plan FAILED", which is a literal attention line. Both off, on
    // purpose; the credential rules stay on because this text is persisted.
    expect(briefingHarness.guard?.rules).toEqual(['secret_leak', 'pii_leak'])
    expect(briefingHarness.guard?.redact).toBe(true)
  })
})

describe('the briefing eval fixtures', () => {
  it('pass on a well-formed briefing', () => {
    for (const e of briefingHarness.evals ?? []) {
      const value = e.input.empty ? 'You are all clear — nothing is waiting on you.' : GOOD_BRIEF
      expect(e.check(value), e.name).toBeNull()
    }
  })

  it('catch the failures they exist for', () => {
    const [shape, grounding, allClear] = briefingHarness.evals ?? []
    expect(shape?.check(['- one', '- two', '- three', '- four', '- five', '- six'].join('\n'))).toContain('over the 5')
    expect(shape?.check('A paragraph with no items at all.')).toContain('no bulleted items')
    expect(grounding?.check('**Unread** — 3 messages in #platform.')).toContain('Ledger migration')
    expect(grounding?.check(`${GOOD_BRIEF}\n- see https://talaria.example/t/41`)).toContain('invented')
    expect(allClear?.check('- Nothing is waiting on you.')).toContain('bulleted list')
    expect(allClear?.check(`All clear. ${'x'.repeat(300)}`)).toContain('one short line')
  })
})

describe('the chat-back definition', () => {
  it('runs zero_tool_claim, unlike the briefing', async () => {
    // The chat CAN act — its prompt permits tools — and the identical question
    // asked in ordinary chat is already checked by `guardChatReply` with this
    // rule on. Leaving it off would make this panel the one place "I've
    // archived those for you" goes unnoticed.
    expect(briefingChatHarness.guard?.rules).toContain('zero_tool_claim')
    // ...and it redacts NOTHING, because it persists nothing. There is no saved
    // copy to clean and the owner already watched the original stream.
    expect(briefingChatHarness.guard?.redact).toBeUndefined()
  })

  it('opens with the briefing as context and ends with what the owner just typed', async () => {
    const { deps: d, requests } = deps('The Ledger migration is the blocked one.')
    const res = await runHarness(
      briefingChatHarness,
      { scope: 'inbox', summary: GOOD_BRIEF, history: [{ role: 'user', content: 'morning' }], content: 'which one is blocked?' },
      { caller: 'test', model: 'penny-assistant', deps: d },
    )
    expect(res.value).toBe('The Ledger migration is the blocked one.')
    const messages = requests[0]?.messages ?? []
    expect(messages[0]?.content).toContain('this thread is NOT saved')
    expect(messages[0]?.content).toContain('Ledger migration')
    expect(messages[1]).toEqual({ role: 'assistant', content: 'Got it.' })
    expect(messages.at(-1)).toEqual({ role: 'user', content: 'which one is blocked?' })
  })

  it('says so plainly before the first briefing exists', async () => {
    const { deps: d, requests } = deps('Nothing to go on yet.')
    await runHarness(briefingChatHarness, { scope: 'plans', summary: null, history: [], content: 'anything?' }, { caller: 'test', model: 'penny-assistant', deps: d })
    expect(requests[0]?.messages[0]?.content).toContain('(none yet)')
  })

  it('keeps only the last twelve turns of history', async () => {
    const history = Array.from({ length: 20 }, (_, i) => ({ role: 'user' as const, content: `turn ${i}` }))
    const { deps: d, requests } = deps('ok')
    await runHarness(briefingChatHarness, { scope: 'inbox', summary: null, history, content: 'now what?' }, { caller: 'test', model: 'penny-assistant', deps: d })
    // 2 framing turns + 12 history + the new one.
    expect(requests[0]?.messages.length).toBe(15)
    expect(requests[0]?.messages[2]?.content).toBe('turn 8')
  })
})

describe('the chat-back eval fixtures', () => {
  it('pass on a well-formed answer', () => {
    const answers = ['The Ledger migration on Platform is the blocked one.', 'Start with the two tickets waiting on your sign-off.']
    briefingChatHarness.evals?.forEach((e, i) => expect(e.check(answers[i] ?? ''), e.name).toBeNull())
  })

  it('catch the failures they exist for', () => {
    const [grounded, short] = briefingChatHarness.evals ?? []
    expect(grounded?.check('Nothing seems to be blocked right now.')).toContain('Ledger migration')
    // The floor, not merely a length check: this fixture used to reject only
    // replies under 10 characters, which let a two-word non-answer score a pass
    // on a question about tickets waiting for a sign-off.
    expect(short?.check('hi')).toContain('too short')
    expect(short?.check('Have a lovely afternoon, and shout if you need me.')).toContain('never engages')
    // Long AND on-topic, so it is the ceiling that rejects it and not the floor.
    expect(short?.check('Review the two waiting tickets. '.repeat(100))).toContain('short, direct')
  })
})
