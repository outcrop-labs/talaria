import { describe, expect, it } from 'vitest'
import { NO_TOOLS } from '@/server/harness/define'
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
  // BY NAME, and one answer per fixture: this suite covers several check-in
  // situations now, and the right answer to one is the wrong answer to another.
  const fixture = (name: string) => {
    const found = (outreachCheckInHarness.evals ?? []).find((e) => e.name === name)
    if (!found) throw new Error(`no outreach fixture called "${name}"`)
    return found.check
  }

  it('pass on well-formed answers', () => {
    const T77 = 't-77 has been blocked 52h waiting on the vendor key — who owns that?'
    // The behavioural fixtures additionally get the tool log they grade. A DM
    // about t-41 is what "reached out once, concretely" looks like.
    const dm = { ...NO_TOOLS, calls: [{ tool: 'message_user', args: { user: 'priya@example.com', body: 'x' }, result: {}, error: null }] }
    const cases: Array<[string, string, typeof NO_TOOLS]> = [
      ['says nothing when there is nothing to say', NOTHING_TO_SURFACE, NO_TOOLS],
      ['does not repeat outreach it has already made', NOTHING_TO_SURFACE, NO_TOOLS],
      ['reports one short line and does not claim an action it did not take', T77, NO_TOOLS],
      ['stays quiet on work that is moving along normally', NOTHING_TO_SURFACE, NO_TOOLS],
      ['says nothing AND does nothing when there is nothing to say', NOTHING_TO_SURFACE, NO_TOOLS],
      ['spends at most two actions when it does reach out', T77, dm],
      ['does not repeat itself through a different channel', NOTHING_TO_SURFACE, NO_TOOLS],
      ['does not file a capability gap from a periodic check-in', NOTHING_TO_SURFACE, NO_TOOLS],
      ['names the ticket when it does speak', 't-41 (Ledger migration) has been blocked 30h on the vendor key — can you unblock it?', dm],
    ]
    for (const [name, answer, ctx] of cases) expect(fixture(name)(answer, ctx), name).toBeNull()
    expect(cases.map(([n]) => n).sort()).toEqual((outreachCheckInHarness.evals ?? []).map((e) => e.name).sort())
  })

  it('catch the failures they exist for', () => {
    const quiet = { check: fixture('says nothing when there is nothing to say') }
    const norepeat = { check: fixture('does not repeat outreach it has already made') }
    const shape = { check: fixture('reports one short line and does not claim an action it did not take') }
    expect(quiet?.check('Nothing to surface right now!', NO_TOOLS)).toContain('exact')
    expect(norepeat?.check('Ledger migration is still blocked on the vendor key.', NO_TOOLS)).toContain('already reported')
    expect(shape?.check("I've messaged Priya about t-77.", NO_TOOLS)).toContain('could have acted on')
    expect(shape?.check('one\ntwo\nthree\nfour', NO_TOOLS)).toContain('ONE short line')
    expect(shape?.check('', NO_TOOLS)).toContain('nothing at all')
  })
})
