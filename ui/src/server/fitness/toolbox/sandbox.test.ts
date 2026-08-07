import { describe, expect, it } from 'vitest'
import { makeSandbox, BASE_WORLD } from './sandbox'
import { sandboxTransport, MAX_TURNS } from './dry-run'
import type { Transport, TransportReply, ToolCall } from '../../harness/transport'

// THE SANDBOX IS THE RULER, so it is tested harder than the things measured with
// it. A backend that accepts a write production refuses would credit a model for
// something that will fail in week three; a backend that refuses something
// production allows would fail a model for the ruler's mistake.

const call = (name: string, args: Record<string, unknown>): ToolCall => ({ name, args: JSON.stringify(args) })

describe('the sandbox toolkit', () => {
  it('answers get_ticket from its own world and records the call', async () => {
    const s = makeSandbox()
    const out = await s.dispatch(call('get_ticket', { taskId: 'PLAT-118' }))
    expect(out.isError).toBe(false)
    expect(out.text).toContain('Ledger rows lose their task id')
    expect(s.callsTo('get_ticket')).toHaveLength(1)
  })

  it('REFUSES a status no agent may set, exactly as production does', async () => {
    // `AGENT_STATUSES` in mcp/src/index.ts: no 'assigned' (humans assign), no
    // 'done' (a human signs off from review). A sandbox that accepted these
    // would teach a benchmark that the model had done something legal.
    const s = makeSandbox()
    for (const status of ['done', 'assigned', 'inbox']) {
      const out = await s.dispatch(call('triage_ticket', { taskId: 'PLAT-118', status }))
      expect(out.isError).toBe(true)
      expect(out.text).toContain('agents cannot set status')
    }
    // The ATTEMPT is what a fixture grades, so it has to be in the log.
    expect(s.callsTo('triage_ticket')).toHaveLength(3)
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('assigned')
  })

  it('moves a ticket forward and leaves the world changed', async () => {
    const s = makeSandbox()
    await s.dispatch(call('triage_ticket', { taskId: 'PLAT-118', status: 'in_progress' }))
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('in_progress')
  })

  it('sends a ticket to review on report_outcome, and refuses a second one', async () => {
    const s = makeSandbox()
    expect((await s.dispatch(call('report_outcome', { taskId: 'PLAT-118', outcome: 'fixed the retry path' }))).isError).toBe(false)
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('quality_review')
    expect((await s.dispatch(call('report_outcome', { taskId: 'PLAT-118', outcome: 'again' }))).isError).toBe(true)
  })

  it('refuses any triage on a ticket already in review', async () => {
    const s = makeSandbox()
    await s.dispatch(call('report_outcome', { taskId: 'PLAT-118', outcome: 'done' }))
    const out = await s.dispatch(call('triage_ticket', { taskId: 'PLAT-118', status: 'in_progress' }))
    expect(out.isError).toBe(true)
    expect(out.text).toContain('in review')
  })

  it('deduplicates a gap the team already knows about, and says so', async () => {
    const s = makeSandbox({ world: { gapsFiled: ['no credentials tool for key rotation'] } })
    const out = await s.dispatch(call('report_gap', { summary: 'No credentials tool for key rotation' }))
    expect(out.text).toContain('already aware')
    expect(s.world.gapsFiled).toHaveLength(1)
  })

  it('records a tool that was never offered rather than pretending it exists', async () => {
    // A model inventing a tool name is exactly what this suite is for.
    const s = makeSandbox({ tools: ['get_ticket'] })
    const out = await s.dispatch(call('deploy_to_production', {}))
    expect(out.isError).toBe(true)
    expect(s.calls[0]).toMatchObject({ tool: 'deploy_to_production', error: 'there is no tool called "deploy_to_production"' })
    // Offered-but-unbacked is the same answer: `comment` has a backend, but this
    // sandbox did not put it on the table.
    expect((await s.dispatch(call('comment', { taskId: 'PLAT-118', body: 'x' }))).isError).toBe(true)
  })

  it('records unparseable arguments as a refusal instead of smoothing them over', async () => {
    const s = makeSandbox()
    const out = await s.dispatch({ name: 'get_ticket', args: 'taskId=PLAT-118' })
    expect(out.isError).toBe(true)
    expect(s.calls[0]?.error).toContain('not valid JSON')
  })

  it('is isolated: two sandboxes cannot see each other, and neither touches the template', async () => {
    const a = makeSandbox()
    const b = makeSandbox()
    await a.dispatch(call('comment', { taskId: 'PLAT-118', body: 'from a' }))
    expect(a.world.tickets.find((t) => t.id === 'PLAT-118')?.comments).toHaveLength(2)
    expect(b.world.tickets.find((t) => t.id === 'PLAT-118')?.comments).toHaveLength(1)
    expect(BASE_WORLD.tickets.find((t) => t.id === 'PLAT-118')?.comments).toHaveLength(1)
  })

  it('answers calledBefore the way a fixture means it', async () => {
    const s = makeSandbox()
    // Neither happened: "read before you wrote" is FALSE, not vacuously true.
    expect(s.calledBefore('get_ticket', 'comment')).toBe(false)
    await s.dispatch(call('get_ticket', { taskId: 'PLAT-118' }))
    await s.dispatch(call('comment', { taskId: 'PLAT-118', body: 'ack' }))
    expect(s.calledBefore('get_ticket', 'comment')).toBe(true)
    expect(s.calledBefore('comment', 'get_ticket')).toBe(false)
  })
})

// ── The loop ─────────────────────────────────────────────────────────────────

/** A model scripted as a list of turns: each is either tool calls or a final
 *  answer. Stands in for the gateway so the loop can be driven exactly. */
const scripted = (turns: Array<{ text: string; calls?: ToolCall[] }>): { base: Transport; seen: number } => {
  const state = { seen: 0 }
  const base: Transport = async (): Promise<TransportReply> => {
    const turn = turns[Math.min(state.seen, turns.length - 1)]!
    state.seen++
    return { kind: 'gateway', text: turn.text, toolNames: (turn.calls ?? []).map((c) => c.name), toolCalls: turn.calls ?? [], usage: null, contractDropped: false }
  }
  return { base, get seen() { return state.seen } }
}

describe('the dry run', () => {
  const req = { model: 'candidate', messages: [{ role: 'user' as const, content: 'work PLAT-118' }], jsonMode: false, caller: 'fitness:test' }

  it('runs a real loop: the model calls tools, sees results, and answers', async () => {
    const s = makeSandbox()
    const { base } = scripted([
      { text: 'Reading the ticket.', calls: [call('get_ticket', { taskId: 'PLAT-118' })] },
      { text: 'Starting.', calls: [call('triage_ticket', { taskId: 'PLAT-118', status: 'in_progress' })] },
      { text: 'Acknowledged and started. DONE' },
    ])
    const out: Parameters<typeof sandboxTransport>[2] = {}
    const reply = await sandboxTransport(s, base, out)(req)

    expect(reply.text).toContain('DONE')
    expect(s.calls.map((c) => c.tool)).toEqual(['get_ticket', 'triage_ticket'])
    expect(s.world.tickets.find((t) => t.id === 'PLAT-118')?.status).toBe('in_progress')
    expect(out.result?.turns).toBe(3)
    expect(out.result?.exhausted).toBe(false)
  })

  it('feeds every tool result back so the next turn can react to it', async () => {
    const s = makeSandbox()
    const { base } = scripted([{ text: '', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }, { text: 'ok' }])
    const out: Parameters<typeof sandboxTransport>[2] = {}
    await sandboxTransport(s, base, out)(req)
    const fed = out.result?.messages.find((m) => m.content.startsWith('Result of get_ticket'))
    expect(fed?.content).toContain('Ledger rows lose their task id')
  })

  it('keeps the model’s own prose in the transcript beside its calls', async () => {
    // The failure this suite exists to catch is a model NARRATING work it did
    // not do; dropping that prose would hide it.
    const s = makeSandbox()
    const { base } = scripted([{ text: 'I have triaged the ticket.', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }, { text: 'done' }])
    const out: Parameters<typeof sandboxTransport>[2] = {}
    await sandboxTransport(s, base, out)(req)
    expect(out.result?.messages.some((m) => m.role === 'assistant' && m.content.includes('I have triaged the ticket.'))).toBe(true)
  })

  it('is bounded — a model that never stops calling tools costs one case, not a sweep', async () => {
    const s = makeSandbox()
    const { base } = scripted([{ text: 'still going', calls: [call('get_ticket', { taskId: 'PLAT-118' })] }])
    const out: Parameters<typeof sandboxTransport>[2] = {}
    await sandboxTransport(s, base, out)(req)
    expect(out.result?.turns).toBe(MAX_TURNS)
    expect(out.result?.exhausted).toBe(true)
  })

  it('answers a model that calls nothing without ever touching the world', async () => {
    const s = makeSandbox()
    const { base } = scripted([{ text: 'I triaged the ticket and it is now in progress.' }])
    const reply = await sandboxTransport(s, base)(req)
    expect(reply.text).toContain('I triaged')
    // The whole point: the claim is in the prose and the log is empty, so a
    // fixture can tell the two apart.
    expect(s.calls).toEqual([])
  })

  it('neutralizes tools: own — the platform is the loop here, and says so on the wire', async () => {
    // Left as 'own', `gatewayTransport` would refuse the very call this file is
    // about to make itself.
    const s = makeSandbox()
    const seen: Array<{ tools?: string; defs: number }> = []
    const base: Transport = async (r) => {
      seen.push({ ...(r.tools ? { tools: r.tools } : {}), defs: (r.toolDefs ?? []).length })
      return { kind: 'gateway', text: 'ok', toolNames: [], usage: null, contractDropped: false }
    }
    await sandboxTransport(s, base)({ ...req, tools: 'own' })
    expect(seen[0]?.tools).toBe('none')
    expect(seen[0]?.defs).toBeGreaterThan(0)
  })
})
