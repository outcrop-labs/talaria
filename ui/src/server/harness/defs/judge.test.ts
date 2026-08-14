import { describe, expect, it } from 'vitest'
import { NO_TOOLS } from '@/server/harness/define'
import { runHarness, type HarnessDeps, type TransportRequest } from '@/server/harness/run'
import { judgeHarness, JUDGE_VERDICT, type JudgeInput, type JudgeVerdict } from '@/server/harness/defs/judge'
import type { Capability } from '@/server/harness/capability'
import { runGuardrails, type Finding, type GuardConfig } from '@/server/guardrails'

// The judge is the harness whose failure is most expensive: in enforcing mode —
// the default — every escalation notifies the board's editors, so a verdict
// nobody can parse is a notification storm rather than an error. These cases
// hold that behavior still against RECORDED REPLIES, including the three shapes
// the old greedy `/\{[\s\S]*\}/` extractor was verified to fail on.
//
// Nothing here touches a database, a gateway or a fleet: every edge the runner
// has is injected.

const GUARD: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

interface World {
  replies?: string[]
  facts?: Partial<Record<Capability, boolean>>
}

function world(w: World = {}) {
  const requests: TransportRequest[] = []
  const recorded: Finding[] = []
  const replies = w.replies ?? ['{"verdict":"pass","summary":"the outcome names the test and the file"}']
  const facts: Partial<Record<Capability, { value: boolean; source: 'probe' }>> = {}
  // `source: 'probe'` is load-bearing, not decoration: widening honors a fact
  // Talaria MEASURED and ignores a declared or learned one, so an unsourced
  // fact here would quietly stop widening anything.
  for (const [cap, value] of Object.entries(w.facts ?? {})) facts[cap as Capability] = { value, source: 'probe' }

  const deps: Partial<HarnessDeps> = {
    resolveModel: async () => ({ model: 'pl-main', step: 'first-routable' }),
    routing: async (model) => ({ endpoints: ['spark'], upstreamModel: model }),
    // capability.ts's cardinal rule: only a fact that positively says "no"
    // counts as missing. Unknown is not missing.
    missingCapabilities: async (_key, required) => required.filter((c) => facts[c]?.value === false),
    capabilities: async () => facts,
    transport: async (req) => {
      requests.push(req)
      return { kind: 'gateway', text: replies[Math.min(requests.length - 1, replies.length - 1)] ?? '', toolNames: [], usage: null, contractDropped: false }
    },
    guardConfig: async () => GUARD,
    guardText: async () => [],
    recordFindings: async (findings) => {
      recorded.push(...findings)
    },
    recordRun: async () => {},
    now: () => 0,
  }
  return { requests, recorded, deps }
}

const TICKET: JudgeInput = {
  title: 'Fix the timezone drift on the weekly digest',
  description: 'Send at 09:00 local, not 09:00 UTC. Add a test.',
  outcome: 'Resolved the org timezone before computing the send window. Added tests for America/Chicago and Asia/Tokyo.',
}

const run = (input: JudgeInput, w: ReturnType<typeof world>) => runHarness(judgeHarness, input, { caller: 'platform:judge', deps: w.deps })

describe('the judge harness', () => {
  it('reads a verdict out of the reply shape the old extractor died on', async () => {
    // Fenced object, then prose containing a brace. First-brace-to-last-brace
    // swallowed the trailing sentence and threw, which became `escalate` on a
    // ticket that had just been judged "pass".
    const w = world({
      replies: ['Here is my assessment:\n\n```json\n{"verdict":"pass","summary":"Both timezones are covered.","issues":[]}\n```\n\nLet me know if you want the {section} breakdown.'],
    })
    const res = await run(TICKET, w)
    expect(res.value).toEqual({ verdict: 'pass', summary: 'Both timezones are covered.', issues: [] })
    expect(res.schemaValid).toBe(true)
    expect(res.escalate).toBe(false)
  })

  it('asks at temperature 0, in JSON mode, with the anchor in the prompt', async () => {
    const w = world()
    await run(TICKET, w)
    expect(w.requests[0]?.temperature).toBe(0)
    expect(w.requests[0]?.jsonMode).toBe(true)
  })

  it('repairs a verdict outside the vocabulary instead of escalating it', async () => {
    // The old parser silently coerced anything that was not "pass"/"revise" to
    // "escalate" — so a model answering "needs-work" pulled a human in. Now it
    // gets one concrete correction first.
    const w = world({ replies: ['{"verdict":"needs-work","summary":"the test is missing"}', '{"verdict":"revise","summary":"the test is missing","issues":["No test was added."]}'] })
    const res = await run(TICKET, w)
    expect(res.repairs).toBe(1)
    expect(res.value?.verdict).toBe('revise')
    expect(w.requests[1]?.messages.at(-1)?.content).toContain("field 'verdict'")
  })

  it('flags an unreadable verdict for escalation rather than returning nothing', async () => {
    // THE load-bearing failure semantic: a verdict nobody could read must reach
    // a person. The runner raises a flag; judge.ts turns it into the escalation
    // row and the notification.
    const w = world({ replies: ['I would need to see the diff before I could judge this.', 'As I said, I cannot judge this without the diff.'] })
    const res = await run(TICKET, w)
    expect(res.value).toBeNull()
    expect(res.escalate).toBe(true)
  })

  it('refuses below its floor without calling the model', async () => {
    // The one harness that refuses. A judge that cannot hold a structured
    // verdict escalates every ticket, and every escalation notifies editors.
    const w = world({ facts: { 'json-strict': false } })
    const res = await run(TICKET, w)
    expect(w.requests).toHaveLength(0)
    expect(res.value).toBeNull()
    expect(res.escalate).toBe(false) // the gate did not run; it did not disagree
    expect(res.error).toContain('json-strict')
  })
})

describe('the judge guard pass', () => {
  // A verdict is a DESCRIPTION of claimed work, which is the one output shape
  // the claim-detecting rules cannot be right about. The judge was the only
  // harness declaring no `guard` block, and an omitted block is not neutral —
  // `narrowGuardConfig` hands back the FULL config — so every one of these
  // findings was landing in `guard_findings` under the judging model's name and
  // inflating the per-model confabulation rate the fitness page reads.
  const CLAIMY =
    '{"verdict":"pass","summary":"The agent updated the ticket and sent the digest email, and the tests it added cover both timezones.","issues":[]}'

  it('would trip zero_tool_claim if every rule ran, so the fixture is real bait', () => {
    const hits = runGuardrails(
      { answer: CLAIMY, toolRecord: { backingTools: [], resultsText: '', anyError: false, overflowed: false }, userMessage: '', policedHosts: [] },
      GUARD,
    )
    expect(hits.map((f) => f.check)).toContain('zero_tool_claim')
  })

  it('narrows to the two rules a verdict can actually break', () => {
    expect(judgeHarness.guard).toEqual({ rules: ['secret_leak', 'pii_leak'], redact: true })
  })

  it('files nothing on a verdict that reports what the agent claims it did', async () => {
    const w = world({ replies: [CLAIMY] })
    const res = await run(TICKET, w)
    expect(res.value?.verdict).toBe('pass')
    expect(res.findings).toEqual([])
    expect(w.recorded).toEqual([])
  })

  it('scrubs a credential out of the persisted verdict in OBSERVE mode', async () => {
    // The mode governs DISCLOSURE; `redact` governs what Talaria writes down,
    // and a verdict is written down four times (a judge_reviews row, an
    // activity label, the escalation notification, the revision comment handed
    // back to the agent). Gating this on strict would mean the default install
    // is the one that keeps the key. GUARD above is `observe`.
    const key = 'sk-ant-api03-AAAABBBBCCCCDDDDEEEEFFFF'
    const w = world({ replies: [`{"verdict":"revise","summary":"The outcome pasted a live key: ${key}","issues":["Rotate that credential."]}`] })
    const res = await run(TICKET, w)
    expect(res.value?.summary).toContain('[redacted Anthropic key]')
    expect(res.value?.summary).not.toContain(key)
    expect(res.value?.issues).toEqual(['Rotate that credential.'])
    expect(res.findings.map((f) => f.check)).toEqual(['secret_leak'])
  })
})

describe('what the judge is shown', () => {
  it('hands the template over as the rubric and the pre-checks as evidence', async () => {
    const w = world()
    await run(
      {
        ...TICKET,
        template: { name: 'Engineering change', body: '## Problem\n## Verification' },
        preFindings: [{ check: 'secret_leak', message: 'The outcome contains what looks like a live key.' }],
      },
      w,
    )
    const prompt = w.requests[0]?.messages.at(-1)?.content ?? ''
    expect(prompt).toContain('TICKET TEMPLATE ("Engineering change"')
    expect(prompt).toContain('AUTOMATED PRE-CHECKS FLAGGED')
    expect(prompt).toContain('- secret leak: The outcome contains what looks like a live key.')
  })

  it('widens to a section-by-section rubric walk only for a model that earned it', async () => {
    const narrow = world()
    await run(TICKET, narrow)
    expect(narrow.requests[0]?.messages[0]?.content).not.toContain('one section at a time')

    const wide = world({ facts: { 'json-strict': true, 'instruction-following': true, 'long-context': true } })
    const res = await run(TICKET, wide)
    expect(res.widened).toBe(true)
    expect(wide.requests[0]?.messages[0]?.content).toContain('one section at a time')
    // Widening buys rigor, never authority: the vocabulary and the schema are
    // identical on both branches, so nothing downstream has to know which ran.
    expect(wide.requests[0]?.messages[0]?.content).toContain('"pass" | "revise" | "escalate"')
  })
})

describe('the verdict contract', () => {
  const parse = (v: unknown) => JUDGE_VERDICT.safeParse(v)

  it('clamps the summary and the issue list rather than rejecting them', async () => {
    // A five-thousand-character assessment is still a judgement. Failing it over
    // the length would escalate a ticket for a formatting reason.
    const out = parse({ verdict: 'revise', summary: 'x'.repeat(5000), issues: Array.from({ length: 40 }, (_, i) => `issue ${i}`) })
    expect(out.success).toBe(true)
    expect(out.data?.summary).toHaveLength(4000)
    expect(out.data?.issues).toHaveLength(20)
  })

  it('coerces and drops issue members the way the parser it replaces did', async () => {
    const out = parse({ verdict: 'revise', summary: 's', issues: ['a real issue', 42, '', null] })
    expect(out.data?.issues).toEqual(['a real issue', '42', 'null'])
  })

  it('accepts a verdict with no issues key at all', async () => {
    expect(parse({ verdict: 'pass', summary: 'fine' }).data).toEqual({ verdict: 'pass', summary: 'fine', issues: [] })
  })

  it('rejects a verdict outside the vocabulary and a verdict with no reasoning', async () => {
    expect(parse({ verdict: 'looks-fine', summary: 's' }).success).toBe(false)
    expect(parse({ verdict: 'pass' }).success).toBe(false)
  })
})

describe('the eval fixtures', () => {
  const evals = judgeHarness.evals ?? []
  const verdictOf = (verdict: JudgeVerdict['verdict'], issues: string[] = ['a named gap']): JudgeVerdict => ({ verdict, summary: 's', issues })

  it('is a labeled set, scored by agreement rather than by another model', () => {
    expect(evals).toHaveLength(12)
    expect(new Set(evals.map((e) => e.name)).size).toBe(12)
  })

  it('scores a perfect judge at zero complaints and a lazy one at several', () => {
    // The check that matters is the one nobody writes: a model that answers
    // "revise" to everything looks excellent on planted gaps and is useless,
    // because in enforcing mode it bounces finished work back forever.
    const alwaysRevise = evals.filter((e) => e.check(verdictOf('revise'), NO_TOOLS) !== null)
    // The five satisfied cases, PLUS the leaked-secret fixture: that one accepts
    // either "revise" or "escalate" but requires the judge to say what it saw,
    // and a model reflexively answering "revise" with a boilerplate issue has
    // not seen anything. A reflexive verdict failing a fixture about naming the
    // evidence is the point of it.
    expect(alwaysRevise).toHaveLength(6)

    const alwaysPass = evals.filter((e) => e.check(verdictOf('pass'), NO_TOOLS) !== null)
    expect(alwaysPass).toHaveLength(7) // the planted gaps and both ambiguous ones
  })

  it('every SATISFIED fixture names a check, because the prompt says to revise without one', () => {
    // THE INVARIANT THIS LOCKS, and the bug it is the fix for. `SYSTEM` ends
    // "prefer 'revise' over 'pass' when the outcome is vague, unverifiable, or
    // skips a requirement", and the widened branch adds that an outcome which
    // "asserts completion in general terms" is not verifiable. A `satisfied:`
    // fixture whose outcome names no check is therefore asking the judge to
    // disobey us, and then scoring it for obeying: four models failed the small
    // -ticket fixture, all four citing the missing evidence.
    //
    // So the fixtures and the prompt are held together here rather than being
    // rediscovered one sweep at a time. This is a claim about OUR fixtures — it
    // does not soften a single verdict.
    const evidence = /test|suite|grep|query plan|passes|passing|verified|confirmed|checked|ran /i
    const unverifiable = evals.filter((e) => e.name.startsWith('satisfied:')).filter((e) => !evidence.test((e.input as { outcome: string }).outcome))
    expect(unverifiable.map((e) => e.name)).toEqual([])
  })

  it('will not accept a "revise" that never says what is wrong', () => {
    const gap = evals.find((e) => e.name.startsWith('gap: a required deliverable'))
    expect(gap?.check(verdictOf('revise', []), NO_TOOLS)).toContain('empty issues list')
  })
})
