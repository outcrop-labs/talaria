import { describe, expect, it } from 'vitest'
import { NOTHING_TO_SURFACE, outreachCheckInHarness } from '@/server/harness/defs/outreach'
import { runHarness, type HarnessDeps, type TransportRequest } from '@/server/harness/run'

const WORK = [
  { id: 't-77', title: 'Vendor webhook signature check', status: 'blocked', board: 'Platform', idleHours: 52 },
  { id: 't-78', title: 'Backfill the audit log', status: 'in_progress', board: 'Platform', idleHours: 3 },
]

function deps(
  reply: string,
  opts: { capable?: boolean; toolNames?: string[] } = {},
): { deps: Partial<HarnessDeps>; requests: TransportRequest[] } {
  const requests: TransportRequest[] = []
  return {
    requests,
    deps: {
      routing: async (model: string) => ({ endpoints: ['fleet-1'], upstreamModel: model }),
      missingCapabilities: async () => [],
      // Only a PROBE fact widens (run.ts step 3), so the shape matters here.
      capabilities: async () => (opts.capable ? { 'instruction-following': { value: true, source: 'probe' as const } } : {}),
      transport: async (req: TransportRequest) => {
        requests.push(req)
        return { kind: 'fleet' as const, text: reply, toolNames: opts.toolNames ?? [], usage: null, contractDropped: false }
      },
      guardConfig: async () => ({ mode: 'observe' as const, checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }),
      guardText: async () => [],
      recordFindings: async () => {},
      recordRun: async () => {},
      now: () => 0,
    },
  }
}

describe('the check-in definition', () => {
  it('runs on the agent’s own model and declares no chain behind it', () => {
    // There is no `platform_agent_models` slot for outreach and there should not
    // be one: this is Dex's check-in on Dex's work, and a fallback model would
    // produce a check-in in a voice that is not the agent's.
    expect(outreachCheckInHarness.model.chain).toEqual([])
    expect(outreachCheckInHarness.model.pin).toBeUndefined()
  })

  it('sends the agent its own tickets and its own recent outreach', async () => {
    const { deps: d, requests } = deps('Flagged t-77 to Priya — blocked 52h on the vendor key.')
    const res = await runHarness(
      outreachCheckInHarness,
      { work: WORK, recent: [{ kind: 'dm', note: 'asked Ana about the rota' }] },
      { caller: 'test', model: 'dex-developer', deps: d },
    )
    expect(res.value).toBe('Flagged t-77 to Priya — blocked 52h on the vendor key.')
    const sent = requests[0]?.messages[0]?.content ?? ''
    expect(sent).toContain('[Automated periodic check-in — no human sent this message.]')
    expect(sent).toContain('- [blocked] "Vendor webhook signature check" (board Platform, ticket t-77, idle 52h)')
    expect(sent).toContain('- dm: asked Ana about the rota')
    expect(sent).toContain(`reply exactly: ${NOTHING_TO_SURFACE}`)
  })

  it('says so plainly when the agent has nothing on its plate', async () => {
    const { deps: d, requests } = deps(NOTHING_TO_SURFACE)
    await runHarness(outreachCheckInHarness, { work: [], recent: [] }, { caller: 'test', model: 'dex-developer', deps: d })
    const sent = requests[0]?.messages[0]?.content ?? ''
    expect(sent).toContain('(no assigned tickets)')
    expect(sent).toContain('(none)')
  })

  it('lands a silent turn on the exact token the sweep filters on', async () => {
    // `text.trim() || NOTHING`, preserved as a declared fallback. The run is
    // still recorded as a contract MISS — the fallback is the caller's safe
    // answer, not evidence the model produced one — so the fitness matrix sees
    // a model that cannot hold the "reply with exactly X" instruction.
    const { deps: d } = deps('   ')
    const res = await runHarness(outreachCheckInHarness, { work: WORK, recent: [] }, { caller: 'test', model: 'dex-developer', deps: d })
    expect(res.value).toBe(NOTHING_TO_SURFACE)
    expect(res.schemaValid).toBe(false)
  })

  it('asks for concrete detail only from a model proven to follow instructions', async () => {
    const narrow = deps('ok')
    const wide = deps('ok', { capable: true })
    const input = { work: WORK, recent: [] }
    const a = await runHarness(outreachCheckInHarness, input, { caller: 'test', model: 'dex-developer', deps: narrow.deps })
    const b = await runHarness(outreachCheckInHarness, input, { caller: 'test', model: 'dex-developer', deps: wide.deps })
    expect(a.widened).toBe(false)
    expect(b.widened).toBe(true)
    expect(narrow.requests[0]?.messages[0]?.content).not.toContain('Be concrete')
    expect(wide.requests[0]?.messages[0]?.content).toContain('Be concrete')
    // The widening is additive: every original rule survives it, so the
    // un-widened branch is exactly today's prompt.
    expect(wide.requests[0]?.messages[0]?.content).toContain('At most 2 actions.')
  })

  it('flags an outreach line that claims an action no tool performed', async () => {
    // The whole reason this harness exists (audit 1.5): `zero_tool_claim` is
    // written for exactly this output and had never run on it.
    const { deps: d } = deps("I've messaged Priya and updated the ticket.", { toolNames: [] })
    const res = await runHarness(outreachCheckInHarness, { work: WORK, recent: [] }, { caller: 'test', model: 'dex-developer', deps: d })
    expect(res.findings.map((f) => f.check)).toContain('zero_tool_claim')
  })

  it('does not flag the same line when the agent really did message someone', async () => {
    const { deps: d } = deps("I've messaged Priya and updated the ticket.", { toolNames: ['message_user', 'comment'] })
    const res = await runHarness(outreachCheckInHarness, { work: WORK, recent: [] }, { caller: 'test', model: 'dex-developer', deps: d })
    expect(res.findings.map((f) => f.check)).not.toContain('zero_tool_claim')
  })

  it('claims no coverage it cannot honestly supply', () => {
    // The persona's tool loop ran inside the agent container, so the runner
    // holds tool NAMES and neither results nor error detail. `ungrounded_ref`
    // and `fabricated_outage` would be skipped anyway; declaring them would
    // read as protection that is not there.
    expect(outreachCheckInHarness.guard?.rules).toEqual(['zero_tool_claim', 'secret_leak', 'pii_leak'])
    expect(outreachCheckInHarness.guard?.redact).toBe(true)
  })
})

describe('the check-in eval fixtures', () => {
  it('pass on well-formed answers', () => {
    const answers = [NOTHING_TO_SURFACE, NOTHING_TO_SURFACE, 't-77 has been blocked 52h waiting on the vendor key — who owns that?']
    outreachCheckInHarness.evals?.forEach((e, i) => expect(e.check(answers[i] ?? ''), e.name).toBeNull())
  })

  it('catch the failures they exist for', () => {
    const [quiet, norepeat, shape] = outreachCheckInHarness.evals ?? []
    expect(quiet?.check('Nothing to surface right now!')).toContain('exact')
    expect(norepeat?.check('Ledger migration is still blocked on the vendor key.')).toContain('already reported')
    expect(shape?.check("I've messaged Priya about t-77.")).toContain('could have acted on')
    expect(shape?.check('one\ntwo\nthree\nfour')).toContain('ONE short line')
    expect(shape?.check('')).toContain('nothing at all')
  })
})
