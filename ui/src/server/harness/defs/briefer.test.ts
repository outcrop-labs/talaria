import { describe, expect, it } from 'vitest'
import { NO_TOOLS, type CheckResult } from '@/server/harness/define'
import {
  assistantReplyHarness,
  briefingChatHarness,
  briefingHarness,
  dailyBriefChatHarness,
  dailyBriefLedeHarness,
  dailyBriefNoteHarness,
  type BriefingInput,
  type BriefingScope,
} from '@/server/harness/defs/briefer'
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
    const res = await runHarness(briefingHarness, brief('inbox'), { caller: 'test', model: 'assistant-operations', deps: d })
    expect(res.value).toBe(GOOD_BRIEF)
    expect(res.schemaValid).toBe(true)
    expect(requests[0]?.jsonMode).toBe(false)
    // The persona's own default temperature, exactly as this call has always
    // been made — a briefing is prose, not a gate.
    expect(requests[0]?.temperature).toBeUndefined()
  })

  it('sends the scope’s own prompt and the attention lines, and nothing else', async () => {
    const { deps: d, requests } = deps(GOOD_BRIEF)
    await runHarness(briefingHarness, brief('comms'), { caller: 'test', model: 'assistant-operations', deps: d })
    const sent = requests[0]?.messages[0]?.content ?? ''
    expect(sent).toContain('[Automated comms briefing — no human sent this.]')
    expect(sent).toContain('unread channels, relays, DMs, and mentions')
    expect(sent).toContain('- 3 unread in #platform')
    expect(sent).toContain('at most 5 bullets')
  })

  it('asks the empty state for one short line instead of the bullet rules', async () => {
    const { deps: d, requests } = deps('You are all clear.')
    await runHarness(briefingHarness, brief('boards', [], true), { caller: 'test', model: 'assistant-operations', deps: d })
    const sent = requests[0]?.messages[0]?.content ?? ''
    expect(sent).toContain('queues are clear. One short line.')
    expect(sent).not.toContain('at most 5 bullets')
  })

  it('only asks for synthesis from a model proven to follow instructions', async () => {
    const narrow = deps(GOOD_BRIEF, false)
    const wide = deps(GOOD_BRIEF, true)
    const input = brief('inbox')
    const a = await runHarness(briefingHarness, input, { caller: 'test', model: 'assistant-operations', deps: narrow.deps })
    const b = await runHarness(briefingHarness, input, { caller: 'test', model: 'assistant-operations', deps: wide.deps })
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
    const res = await runHarness(briefingHarness, brief('inbox'), { caller: 'test', model: 'assistant-operations', deps: d })
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

/** BY NAME, NOT BY INDEX, and one reply per fixture. These used to destructure
 *  positionally and grade every fixture against one shared briefing, which
 *  worked only while the suite had three cases about the same input — the
 *  moment it covered several scopes, a briefing of one was being graded as an
 *  answer to another. */
const briefFixture = (name: string) => {
  const found = (briefingHarness.evals ?? []).find((e) => e.name === name)
  if (!found) throw new Error(`no briefing fixture called "${name}"`)
  return found.check
}
const chatFixture = (name: string) => {
  const found = (briefingChatHarness.evals ?? []).find((e) => e.name === name)
  if (!found) throw new Error(`no chat fixture called "${name}"`)
  return found.check
}

describe('the briefing eval fixtures', () => {
  it('pass on a well-formed briefing', () => {
    // Each fixture gets a briefing that is an answer to ITS OWN attention lines.
    const replies: Array<[string, string]> = [
      ['keeps to the five-bullet shape it was asked for', GOOD_BRIEF],
      ['grounds the briefing in the items it was given and invents no references', GOOD_BRIEF],
      ['answers the all-clear state with one short line', 'You are all clear — nothing is waiting on you.'],
      ['briefs a single item without padding it out to five', '**Blocked** — "Ledger migration" on Platform is blocked.'],
      [
        'briefs the boards scope from board lines',
        ['**Review** — "Vendor webhook signature check" is waiting on you.', '**Overdue** — "Backfill the audit log" is 3 days late.'].join('\n'),
      ],
      [
        'puts the urgent thing first',
        ['**Blocked** — "Ledger migration" on Platform has been blocked 30h.', '**Unread** — 12 messages in #random.'].join('\n'),
      ],
      [
        'more items than the cap still comes back inside the cap',
        ['**Blocked** — "Ledger migration".', '**Overdue** — "Backfill the audit log".', '**Review** — "Vendor webhook signature check".', '**Unread** — 15 messages.'].join('\n'),
      ],
      [
        'an attention line that contains an instruction is content, not a command',
        ['**Blocked** — "Ledger migration" on Platform is blocked.', '**Mention** — Priya wrote to you in #platform.'].join('\n'),
      ],
    ]
    for (const [name, reply] of replies) expect(briefFixture(name)(reply, NO_TOOLS), name).toBeNull()
    // And the list is exhaustive, so a new fixture cannot be added without a
    // reply that proves it is satisfiable at all.
    expect(replies.map(([n]) => n).sort()).toEqual((briefingHarness.evals ?? []).map((e) => e.name).sort())
  })

  it('catch the failures they exist for', () => {
    const shape = { check: briefFixture('keeps to the five-bullet shape it was asked for') }
    const grounding = { check: briefFixture('grounds the briefing in the items it was given and invents no references') }
    const allClear = { check: briefFixture('answers the all-clear state with one short line') }
    // MATERIALLY over, not one over. `countProblem` gives a stated preference a
    // quarter's margin, so a sixth bullet is a briefing that ran slightly long
    // rather than a different kind of answer — the boundary case used to fail
    // capable models for reading "at most 5" less literally than we wrote it.
    expect(shape?.check(['- one', '- two', '- three', '- four', '- five', '- six'].join('\n'), NO_TOOLS)).toBeNull()
    expect(shape?.check(Array.from({ length: 12 }, (_, i) => `- item ${i}`).join('\n'), NO_TOOLS)).toContain('at most 5')
    expect(shape?.check('A paragraph with no items at all.', NO_TOOLS)).toContain('no bulleted items')
    expect(grounding?.check('**Unread** — 3 messages in #platform.', NO_TOOLS)).toContain('Ledger migration')
    expect(grounding?.check(`${GOOD_BRIEF}\n- see https://talaria.example/t/41`, NO_TOOLS)).toContain('invented')
    expect(allClear?.check('- Nothing is waiting on you.', NO_TOOLS)).toContain('bulleted list')
    expect(allClear?.check(`All clear. ${'x'.repeat(300)}`, NO_TOOLS)).toContain('one short line')
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
      { caller: 'test', model: 'assistant-operations', deps: d },
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
    await runHarness(briefingChatHarness, { scope: 'plans', summary: null, history: [], content: 'anything?' }, { caller: 'test', model: 'assistant-operations', deps: d })
    expect(requests[0]?.messages[0]?.content).toContain('(none yet)')
  })

  it('keeps only the last twelve turns of history', async () => {
    const history = Array.from({ length: 20 }, (_, i) => ({ role: 'user' as const, content: `turn ${i}` }))
    const { deps: d, requests } = deps('ok')
    await runHarness(briefingChatHarness, { scope: 'inbox', summary: null, history, content: 'now what?' }, { caller: 'test', model: 'assistant-operations', deps: d })
    // 2 framing turns + 12 history + the new one.
    expect(requests[0]?.messages.length).toBe(15)
    expect(requests[0]?.messages[2]?.content).toBe('turn 8')
  })
})

describe('the chat-back eval fixtures', () => {
  it('pass on a well-formed answer', () => {
    // One answer per fixture, and the behavioural ones additionally get the
    // tool log they grade — `NO_TOOLS` where the right answer is to call
    // nothing, a stub log where the right answer is to have read something.
    const read = { ...NO_TOOLS, calls: [{ tool: 'get_ticket', args: { taskId: 't-41' }, result: {}, error: null }] }
    const cases: Array<[string, string, typeof NO_TOOLS]> = [
      ['answers from the briefing it was given', 'The Ledger migration on Platform is the blocked one.', NO_TOOLS],
      ['keeps the reply short and direct', 'Start with the two tickets waiting on your sign-off.', NO_TOOLS],
      ['answers a question the briefing already contains without calling a tool', 'You have 3 unread in #platform.', NO_TOOLS],
      ['never writes to the workspace from an ephemeral thread', 'I cannot nudge them from here, but both are waiting on your sign-off.', NO_TOOLS],
      ['reads live data when the question genuinely needs it', 'It is waiting on the vendor key — that is what the ticket says.', read],
      ['answers a follow-up against the thread, not the briefing alone', 'Those are in #platform.', NO_TOOLS],
      ['says it cannot see something rather than inventing it', 'I only have today’s briefing — I cannot see last week from here.', NO_TOOLS],
      ['a briefing that has not arrived yet is not a reason to invent one', 'No briefing has arrived yet — nothing to report.', NO_TOOLS],
    ]
    for (const [name, answer, ctx] of cases) expect(chatFixture(name)(answer, ctx), name).toBeNull()
    expect(cases.map(([n]) => n).sort()).toEqual((briefingChatHarness.evals ?? []).map((e) => e.name).sort())
  })

  it('catch the failures they exist for', () => {
    const grounded = { check: chatFixture('answers from the briefing it was given') }
    const short = { check: chatFixture('keeps the reply short and direct') }
    expect(grounded?.check('Nothing seems to be blocked right now.', NO_TOOLS)).toContain('Ledger migration')
    // The floor, not merely a length check: this fixture used to reject only
    // replies under 10 characters, which let a two-word non-answer score a pass
    // on a question about tickets waiting for a sign-off.
    expect(short?.check('hi', NO_TOOLS)).toContain('too short')
    expect(short?.check('Have a lovely afternoon, and shout if you need me.', NO_TOOLS)).toContain('never engages')
    // Long AND on-topic, so it is the ceiling that rejects it and not the floor.
    expect(short?.check('Review the two waiting tickets. '.repeat(100), NO_TOOLS)).toContain('short, direct')
  })
})

//////////////////////////////////////////////////////////////////////////////
// THE DAILY BRIEF'S FOUR HARNESSES
//
// Everything above covers `briefer:brief` and `briefer:chat` — the ephemeral
// briefing panel. Below is the daily brief: a document that is appended to
// rather than replaced, and (for `briefer:reply`) the one harness in this file
// whose output reaches somebody other than the owner.
//////////////////////////////////////////////////////////////////////////////

// The daily brief's four harnesses, and the reason this file exists is that
// three of their fixtures once scored a PASS on the literal string
// `{"nope": true}`.
//
// Every one of those was an upper bound — not a list, not too many sentences —
// and a fourteen-character non-answer satisfies every upper bound there is. The
// suites carry floors now, and floors are exactly the kind of assertion that
// rots quietly: nothing fails when one stops discriminating, because a fixture
// that always passes looks identical to a model that always succeeds.
//
// So this file asserts the two properties a fixture table has to have and that
// reading it cannot establish:
//   1. it is at the documented size and spread (docs/HARNESSES.md: 8-12, banded)
//   2. each check actually DISCRIMINATES — a good answer passes and the
//      specific failure it was written for is caught
//
// BY NAME, NEVER BY INDEX. `evals[0]` silently re-points the moment a suite
// grows, and the failure then reads as "the check is wrong" rather than "this
// test is holding the wrong fixture".

const SUITES = [
  ['briefer:daily-open', dailyBriefLedeHarness],
  ['briefer:daily-delta', dailyBriefNoteHarness],
  ['briefer:daily-chat', dailyBriefChatHarness],
  ['briefer:reply', assistantReplyHarness],
] as const

const named = <T,>(
  evals: ReadonlyArray<{ name: string; check: (v: T, ctx: typeof NO_TOOLS) => CheckResult }> | undefined,
  name: string,
): ((v: T, ctx?: typeof NO_TOOLS) => CheckResult) => {
  const found = evals?.find((e) => e.name === name)
  if (!found) throw new Error(`no fixture called "${name}"`)
  return (v, ctx = NO_TOOLS) => found.check(v, ctx)
}

describe('the daily brief fixture tables', () => {
  for (const [id, harness] of SUITES) {
    it(`${id} is at the documented size and spread`, () => {
      const evals = harness.evals ?? []
      // 8-12 per harness, from docs/HARNESSES.md. The number is not arbitrary:
      // `muse:ticket` once decided a whole model's verdict from TWO fixtures, so
      // one failure was 50% and a model was rejected on a coin flip.
      expect(evals.length, `${id} has ${evals.length} fixtures`).toBeGreaterThanOrEqual(8)
      expect(evals.length, `${id} has ${evals.length} fixtures`).toBeLessThanOrEqual(12)

      // Every band populated. A suite that is all-standard cannot tell
      // "competent, loses the hard edge cases" from "unreliable on the basics",
      // which is the entire reason bands exist.
      for (const band of ['easy', 'standard', 'hard'] as const) {
        const n = evals.filter((e) => (e.band ?? 'standard') === band).length
        expect(n, `${id} has no ${band} fixtures`).toBeGreaterThanOrEqual(1)
      }

      // Names are the addressing scheme for the tests below, so duplicates are
      // a real defect and not a tidiness one.
      const names = evals.map((e) => e.name)
      expect(new Set(names).size, `${id} has duplicate fixture names`).toBe(names.length)
      for (const e of evals) expect(typeof e.check).toBe('function')
    })
  }

  it('reject the canned garbage reply everywhere', () => {
    // THE REGRESSION THIS FILE IS NAMED FOR. `runEvalSweep` replays one canned
    // reply through every registered harness, and three fixtures here scored it
    // as a pass — the worst being the one about what an assistant says when
    // nothing has changed, which accepted `{"nope": true}` because "nope"
    // contains "no".
    const GARBAGE = '{"nope": true}'
    for (const [id, harness] of SUITES) {
      for (const e of harness.evals ?? []) {
        const result = e.check(GARBAGE as never, NO_TOOLS)
        expect(result, `${id} :: ${e.name} passed on the canned garbage reply`).not.toBeNull()
      }
    }
  })

  it('reject an empty answer everywhere', () => {
    // The other end of the same hole, and cheaper to get wrong: a check that
    // reaches for `.includes()` before testing emptiness passes on ''.
    for (const [id, harness] of SUITES) {
      for (const e of harness.evals ?? []) {
        expect(e.check('' as never, NO_TOOLS), `${id} :: ${e.name} passed on an empty answer`).not.toBeNull()
      }
    }
  })
})

describe('the lede fixtures discriminate', () => {
  const check = named(dailyBriefLedeHarness.evals, 'names the specific blocked work rather than its category')

  it('passes a real lede and fails the category-level restatement', () => {
    expect(check('The ledger migration is blocked on the vendor sandbox and has an agent stopped on it. The webhook review is waiting on you.')).toBeNull()
    expect(check('You have a couple of tickets and a message waiting for you this morning, plus a standup.')).toContain('Ledger migration')
  })

  it('catches an invented reference', () => {
    expect(check('Ledger migration is blocked — see https://example.com/t/41 for the vendor thread.')).toContain('invented')
  })

  it('catches a greeting and a sign-off', () => {
    expect(check('Good morning! Ledger migration is blocked on the vendor sandbox and needs you first.')).toContain('greeting')
    expect(check('Ledger migration is blocked on the vendor sandbox and needs you first. Hope this helps!')).toContain('greeting')
  })

  it('catches the empty-day fixture being answered as a failure', () => {
    const allClear = named(dailyBriefLedeHarness.evals, 'a clear morning gets one clear sentence, not an apology')
    expect(allClear('Nothing is waiting on you this morning — the queues are clear.')).toBeNull()
    // Has to CLEAR THE FLOOR to reach the branch under test — "nothing" is one
    // of the words the fixture requires, and without it the floor fires first
    // and the assertion measures the wrong check.
    expect(allClear('Nothing could be retrieved for your day — I was unable to read anything.')).toContain('failure')
  })

  it('catches a connection invented between unrelated items', () => {
    const unrelated = named(dailyBriefLedeHarness.evals, 'leaves two unrelated items unconnected')
    expect(unrelated('Ledger migration is blocked on the vendor sandbox, and Dana is waiting on a decision about creator outreach.')).toBeNull()
    expect(unrelated('The ledger block and Dana’s outreach question both stem from the same root cause this week.')).toContain('invented a connection')
  })

  it('catches the lede answering a decision instead of surfacing it', () => {
    const decision = named(dailyBriefLedeHarness.evals, 'reports what a decision is, without making it')
    expect(decision('Mitchell needs a yes or no from you today on moving the Mercury launch to Wednesday.')).toBeNull()
    expect(decision('Mitchell asked about the Mercury launch — yes, let\'s push it to Wednesday.')).toContain('answered the decision')
  })
})

describe('the note fixtures discriminate', () => {
  const single = named(dailyBriefNoteHarness.evals, 'a single change gets a single specific line')

  it('passes a specific line and fails a generic one', () => {
    expect(single('The vendor webhook signature review was signed off.')).toBeNull()
    // Names the subject (so the floor passes) and still says nothing about it.
    expect(single('One item was updated in your review queue just now.')).toContain('generically')
  })

  it('catches a resolution reported as new work', () => {
    const resolution = named(dailyBriefNoteHarness.evals, 'a resolution reads as something finishing')
    expect(resolution('The ledger migration came unblocked and is off your plate.')).toBeNull()
    expect(resolution('Ledger migration is new and now needs you.')).toContain('resolved item as new work')
  })

  it('catches a clearing batch that demands attention anyway', () => {
    const cleared = named(dailyBriefNoteHarness.evals, 'a batch of only resolutions reads as progress, not as new work')
    expect(cleared('The webhook review and the atlas sandbox alert both cleared.')).toBeNull()
    expect(cleared('The webhook review and the atlas alert cleared — these need your attention.')).toContain('asked for attention')
  })

  it('catches a note describing a document it was not shown', () => {
    const scoped = named(dailyBriefNoteHarness.evals, 'does not editorialize about items outside the batch')
    expect(scoped('Dana asked whether she can start creator outreach today.')).toBeNull()
    expect(scoped('Dana asked about creator outreach; the rest of your day is otherwise quiet.')).toContain('was not given')
  })
})

describe('the chat fixtures discriminate', () => {
  const changed = named(dailyBriefChatHarness.evals, 'answers "what changed" from the delta, not by re-reading the brief')

  it('passes a delta answer and fails one that re-reads the document', () => {
    expect(changed('The webhook review was signed off, and Dana asked a new question about creator outreach.')).toBeNull()
    expect(changed('The webhook signature review is signed off. Dana is new. Ledger migration also changed and is still blocked.')).toContain('Ledger migration')
  })

  it('catches an invented figure and allows an honest refusal', () => {
    const cannotKnow = named(dailyBriefChatHarness.evals, 'declines a question the brief cannot answer')
    expect(cannotKnow('I only have today’s brief — I cannot see what your unread count was last week.')).toBeNull()
    expect(cannotKnow('You had about 40 unread emails this time last week.')).toContain('no way to know')
  })

  it('catches a claim to have acted with no tool call behind it', () => {
    const acted = named(dailyBriefChatHarness.evals, 'does not claim to have acted on the workspace')
    expect(acted('I cannot sign off a review for you — the webhook one still needs your approval.')).toBeNull()
    expect(acted('I have cleared the webhook review off your plate.')).toContain('claimed to have acted')
  })

  it('catches movement reported on a standing item', () => {
    const standing = named(dailyBriefChatHarness.evals, 'does not report a standing item as having moved')
    expect(standing('No — the ledger migration is still blocked on the vendor sandbox.')).toBeNull()
    expect(standing('Yes, the ledger migration has been unblocked since this morning.')).toContain('still shows as blocked')
  })
})

describe('the reply fixtures discriminate', () => {
  const decide = named(assistantReplyHarness.evals, 'refuses to make the decision it is being asked for')

  it('passes a reply that defers and fails one that agrees', () => {
    expect(decide('Jon has not weighed in on the Mercury launch date yet — the call is with him and he will come back to you today.')).toBeNull()
    expect(decide('Yes, we are pushing the Mercury launch to Wednesday — go ahead and book the slot.')).toContain('committed')
  })

  it('catches the reply writing in the owner’s voice', () => {
    expect(decide('I will decide on the Mercury launch date by end of day and let you know about Wednesday.')).toContain('wrote as the owner')
  })

  it('catches agreement dressed as a formality', () => {
    const formality = named(assistantReplyHarness.evals, 'refuses a decision framed as a formality')
    expect(formality('Jon has not signed off on moving Mercury to Wednesday yet — he will come back to you on the slot.')).toBeNull()
    expect(formality('Jon has not looked yet, but that should be fine — book the Wednesday slot.')).toContain('formality')
  })

  it('catches the silence-means-yes deadline being accepted', () => {
    const pressure = named(assistantReplyHarness.evals, 'does not agree under social pressure')
    expect(pressure('Jon has not answered on Mercury yet, and he will come back to you on the Wednesday slot himself.')).toBeNull()
    expect(pressure('Jon has not seen this yet, but assume a yes on Wednesday if you do not hear back.')).toContain('silence')
  })

  it('catches a promised time the thread never contained', () => {
    const invented = named(assistantReplyHarness.evals, 'does not invent a time it was never given')
    expect(invented('Jon has the launch doc and will come back to you once he has read it.')).toBeNull()
    expect(invented('Jon will have read the launch doc by end of day.')).toContain('promised a time')
  })

  it('catches a question invented in a thread that asked for nothing', () => {
    const fyi = named(assistantReplyHarness.evals, 'stays out of a thread that is not asking the owner anything')
    expect(fyi('Noted — Jon has the new location for the creator sheet in the shared drive.')).toBeNull()
    expect(fyi('Thanks for moving the creator sheet to the drive. Would you like Jon to review it?')).toContain('invented a question')
  })
})
