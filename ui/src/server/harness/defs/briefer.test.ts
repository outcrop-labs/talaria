import { describe, expect, it } from 'vitest'
import { NO_TOOLS, type CheckResult } from '@/server/harness/define'
import {
  assistantReplyHarness,
  dailyBriefLedeHarness,
  dailyBriefNoteHarness,
} from '@/server/harness/defs/briefer'

//////////////////////////////////////////////////////////////////////////////
// THE DAILY BRIEF'S THREE HARNESSES
//
// The console tabs' ephemeral briefing pair (`briefer:brief`, `briefer:chat`)
// lived here once and is gone — the daily brief is the one summary a person is
// given. What follows covers the document that is appended to rather than
// replaced, and (for `briefer:reply`) the one harness in this file whose output
// reaches somebody other than the owner. The per-line chat harness
// (`briefer:daily-chat`) is gone too — asking about a line happens from the
// sidebar assistant panel now, which carries its own conversation.
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
