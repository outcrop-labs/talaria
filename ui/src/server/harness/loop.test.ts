import { describe, expect, it, vi } from 'vitest'
import { z } from 'zod'
import type { GuardConfig } from '@/server/guardrails'
import { defineHarness, type HarnessDefinition, type HarnessTool, type HarnessToolContext } from '@/server/harness/define'
import { runHarness, runHarnessStreamed, type HarnessDeps, type LoopEdges, type TransportReply, type TransportRequest } from '@/server/harness/run'

// THE RUNNER-HELD TOOL LOOP (`def.loop`), end to end against scripted model
// turns. The guard and the parser are real; every other edge is recorded.

const ANSWER = z.object({ customer: z.string(), orders: z.number() })
type Answer = z.infer<typeof ANSWER>

interface Script {
  /** The model's turns, in order: text, or tool calls (with optional text). */
  turns: Array<string | { calls: Array<{ name: string; args?: unknown }>; text?: string }>
  guard?: GuardConfig
}

function scripted(s: Script) {
  const requests: TransportRequest[] = []
  const deps: Partial<HarnessDeps> = {
    resolveModel: async () => ({ model: 'gw-model', step: 'pin' }),
    routing: async (m) => ({ endpoints: ['spark'], upstreamModel: m }),
    personaKeys: async () => [],
    missingCapabilities: async () => [],
    capabilities: async () => ({}),
    reach: async () => ({}),
    slotEffort: async () => null,
    transport: async (req): Promise<TransportReply> => {
      requests.push({ ...req, messages: [...req.messages] })
      const turn = s.turns[Math.min(requests.length - 1, s.turns.length - 1)] ?? ''
      if (typeof turn === 'string') return { kind: 'gateway', text: turn, toolNames: [], toolCalls: [], usage: null, contractDropped: false }
      const calls = turn.calls.map((c, i) => ({ name: c.name, args: typeof c.args === 'string' ? c.args : JSON.stringify(c.args ?? {}), id: `call_${requests.length}_${i}` }))
      return { kind: 'gateway', text: turn.text ?? '', toolNames: calls.map((c) => c.name), toolCalls: calls, usage: null, contractDropped: false }
    },
    guardConfig: async () => s.guard ?? { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false },
    guardText: async () => [],
    recordFindings: async () => {},
    recordRun: async () => {},
  }
  return { requests, deps }
}

const findCustomer = vi.fn(async (args: Record<string, unknown>, _ctx: HarnessToolContext) => ({ id: 'c-7', email: args.email }))

const harness = (tools: HarnessTool<{ email: string }>[], maxTurns?: number): HarnessDefinition<{ email: string }, Answer> =>
  defineHarness<{ email: string }, Answer>({
    id: 'app:lookup',
    label: 'Lookup',
    job: 'Looks a customer up and counts their orders.',
    requires: ['json', 'tools'],
    floor: { capabilities: [], refuseBelow: false, note: 'Any model that calls tools.' },
    model: {},
    render: (i) => [
      { role: 'system', content: 'Look the customer up, then count their orders.' },
      { role: 'user', content: i.email },
    ],
    output: { kind: 'json', schema: ANSWER },
    onFailure: 'null',
    loop: { tools, ...(maxTurns ? { maxTurns } : {}) },
  })

const FIND: HarnessTool<{ email: string }> = {
  name: 'find_customer',
  description: 'Find a customer by email.',
  parameters: { type: 'object', properties: { email: { type: 'string' } } },
  handler: findCustomer,
  simulate: (args) => ({ id: 'sim-1', email: args.email }),
}
const ORDERS_MCP: HarnessTool<{ email: string }> = {
  name: 'recent_orders',
  description: "The customer's recent orders.",
  parameters: { type: 'object', properties: { customer: { type: 'string' } } },
  mcp: 'orders_recent',
  simulate: () => ({ count: 2 }),
}
const SEARCH_ORG: HarnessTool<{ email: string }> = {
  name: 'search_docs',
  description: 'Search the docs.',
  parameters: { type: 'object', properties: { q: { type: 'string' } } },
  org: 'docs/search',
}

const run = (def: HarnessDefinition<{ email: string }, Answer>, deps: Partial<HarnessDeps>, loop?: LoopEdges) =>
  runHarness(def, { email: 'ada@example.test' }, { caller: 'test:loop', deps, ...(loop ? { loop } : {}) })

describe('the loop', () => {
  it('offers the tools, dispatches the call, hands the result back on the tool channel, and parses the answer', async () => {
    findCustomer.mockClear()
    const s = scripted({ turns: [{ calls: [{ name: 'find_customer', args: { email: 'ada@example.test' } }] }, '{"customer":"c-7","orders":0}'] })
    const r = await run(harness([FIND]), s.deps, { app: 'support', actingUserId: 'u-1' })
    expect(r.value).toEqual({ customer: 'c-7', orders: 0 })
    expect(r.schemaValid).toBe(true)
    expect(findCustomer).toHaveBeenCalledOnce()
    expect(findCustomer.mock.calls[0]![0]).toEqual({ email: 'ada@example.test' })
    expect(findCustomer.mock.calls[0]![1]).toMatchObject({ app: 'support', harness: 'app:lookup', actingUserId: 'u-1', turn: 0 })
    // The first turn offered the tool; the second saw the call and its result.
    expect(s.requests[0]!.toolDefs?.map((d) => d.name)).toEqual(['find_customer'])
    expect(s.requests[0]!.tools).toBe('none')
    const second = s.requests[1]!.messages
    expect(second.at(-2)).toMatchObject({ role: 'assistant', toolCalls: [{ name: 'find_customer' }] })
    expect(second.at(-1)).toMatchObject({ role: 'tool', toolCallId: 'call_1_0', content: '{"id":"c-7","email":"ada@example.test"}' })
    // The record production and the fixtures share.
    expect(r.toolCalls?.map((c) => c.name)).toEqual(['find_customer'])
    expect(r.calls).toEqual([{ tool: 'find_customer', args: { email: 'ada@example.test' }, result: { id: 'c-7', email: 'ada@example.test' }, error: null }])
    expect(r.exhausted).toBe(false)
  })

  it('honors three calls a turn, and answers a call to a tool it was not offered with an error', async () => {
    const s = scripted({
      turns: [
        { calls: [{ name: 'find_customer' }, { name: 'find_customer' }, { name: 'delete_everything' }, { name: 'find_customer' }] },
        '{"customer":"c-7","orders":1}',
      ],
    })
    const r = await run(harness([FIND]), s.deps)
    expect(r.calls?.map((c) => c.tool)).toEqual(['find_customer', 'find_customer', 'delete_everything'])
    expect(r.calls?.[2]?.error).toMatch(/"delete_everything" is not a tool you have here; your tools are find_customer/)
  })

  it('makes a failure a result the model sees, not a failed run', async () => {
    const boom: HarnessTool<{ email: string }> = { ...FIND, handler: async () => Promise.reject(new Error('CRM is down')) }
    const s = scripted({ turns: [{ calls: [{ name: 'find_customer' }] }, '{"customer":"unknown","orders":0}'] })
    const r = await run(harness([boom]), s.deps)
    expect(r.value).toEqual({ customer: 'unknown', orders: 0 })
    expect(r.calls?.[0]?.error).toBe('tool "find_customer" failed: CRM is down')
    expect(s.requests[1]!.messages.at(-1)).toMatchObject({ role: 'tool', content: 'tool "find_customer" failed: CRM is down' })
  })

  it('runs out of turns: the last calls still dispatch, then one closing turn with the tools taken away', async () => {
    findCustomer.mockClear()
    const s = scripted({ turns: [{ calls: [{ name: 'find_customer' }] }, { calls: [{ name: 'find_customer' }], text: 'one more look' }, '{"customer":"c-7","orders":3}'] })
    const r = await run(harness([FIND], 2), s.deps)
    expect(findCustomer).toHaveBeenCalledTimes(2)
    expect(r.exhausted).toBe(true)
    expect(r.value).toEqual({ customer: 'c-7', orders: 3 })
    const closing = s.requests[2]!
    expect(closing.toolDefs).toBeUndefined()
    expect(closing.messages.at(-1)?.content).toMatch(/run out of turns/)
    expect(closing.messages.at(-1)?.content).toMatch(/Reply with exactly one JSON value/)
  })

  it('a repair turn continues the conversation, with no loop tools offered', async () => {
    const s = scripted({ turns: [{ calls: [{ name: 'find_customer' }] }, 'the customer is c-7', '{"customer":"c-7","orders":0}'] })
    const r = await run(harness([FIND]), s.deps)
    expect(r.repairs).toBe(1)
    expect(r.value?.customer).toBe('c-7')
    const repair = s.requests[2]!
    expect(repair.toolDefs).toBeUndefined()
    expect(repair.messages.some((m) => m.role === 'tool')).toBe(true)
    expect(repair.messages.at(-2)).toMatchObject({ role: 'assistant', content: 'the customer is c-7' })
  })

  it("routes an mcp tool through the host's app dispatch, and says why when there is none", async () => {
    const mcp = vi.fn(async () => ({ text: '{"count":2}', structured: { count: 2 }, isError: false }))
    const s = scripted({ turns: [{ calls: [{ name: 'recent_orders', args: { customer: 'c-7' } }] }, '{"customer":"c-7","orders":2}'] })
    const r = await run(harness([ORDERS_MCP]), s.deps, { app: 'support', actingUserId: null, mcp })
    expect(mcp).toHaveBeenCalledWith('orders_recent', { customer: 'c-7' }, expect.any(AbortSignal))
    expect(r.calls?.[0]).toMatchObject({ tool: 'recent_orders', result: { count: 2 }, error: null })

    const bare = await run(harness([ORDERS_MCP]), scripted({ turns: [{ calls: [{ name: 'recent_orders' }] }, '{"customer":"x","orders":0}'] }).deps)
    expect(bare.calls?.[0]?.error).toMatch(/no app to dispatch it through — run the harness with ctx\.harness/)
  })

  it('calls an org tool as the agent the host provides, split into server and tool, and says why when there is none', async () => {
    const org = vi.fn(async () => ({ text: 'three docs', isError: false }))
    const s = scripted({ turns: [{ calls: [{ name: 'search_docs', args: { q: 'refunds' } }] }, '{"customer":"c-7","orders":0}'] })
    await run(harness([SEARCH_ORG]), s.deps, { app: 'support', actingUserId: null, org })
    expect(org).toHaveBeenCalledWith('docs', 'search', { q: 'refunds' }, expect.any(AbortSignal))

    const none = await run(harness([SEARCH_ORG]), scripted({ turns: [{ calls: [{ name: 'search_docs' }] }, '{"customer":"x","orders":0}'] }).deps, {
      app: 'support',
      actingUserId: 'u-1',
      orgUnavailable: 'org tools run as an agent, and this run was started by a person',
    })
    expect(none.calls?.[0]?.error).toBe('org tools run as an agent, and this run was started by a person')
  })

  it('a dry run answers every tool through simulate and touches nothing real', async () => {
    findCustomer.mockClear()
    const mcp = vi.fn()
    const s = scripted({ turns: [{ calls: [{ name: 'find_customer', args: { email: 'e' } }, { name: 'recent_orders' }, { name: 'search_docs' }] }, '{"customer":"sim-1","orders":2}'] })
    const r = await run(harness([FIND, ORDERS_MCP, SEARCH_ORG]), s.deps, { app: 'support', actingUserId: null, simulate: true, mcp })
    expect(findCustomer).not.toHaveBeenCalled()
    expect(mcp).not.toHaveBeenCalled()
    expect(r.calls?.map((c) => [c.tool, c.result, c.error])).toEqual([
      ['find_customer', { id: 'sim-1', email: 'e' }, null],
      ['recent_orders', { count: 2 }, null],
      ['search_docs', 'tool "search_docs" has no simulate, so a dry run cannot answer it', 'tool "search_docs" has no simulate, so a dry run cannot answer it'],
    ])
  })

  it('grounds the guard in what the tools really returned', async () => {
    const policed: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: ['talaria.internal'], coach: false }
    const linking: HarnessTool<{ email: string }> = { ...FIND, handler: async () => 'Customer page: https://talaria.internal/customers/c7' }
    const grounded = await run(
      { ...harness([linking]), output: { kind: 'text' } } as unknown as HarnessDefinition<{ email: string }, Answer>,
      scripted({ turns: [{ calls: [{ name: 'find_customer' }] }, 'See https://talaria.internal/customers/c7.'], guard: policed }).deps,
    )
    expect(grounded.findings.map((f) => f.check)).not.toContain('ungrounded_ref')

    const invented = await run(
      { ...harness([linking]), output: { kind: 'text' } } as unknown as HarnessDefinition<{ email: string }, Answer>,
      scripted({ turns: [{ calls: [{ name: 'find_customer' }] }, 'See https://talaria.internal/customers/c9999.'], guard: policed }).deps,
    )
    expect(invented.findings.map((f) => f.check)).toContain('ungrounded_ref')
  })

  it('does not stream', async () => {
    const s = scripted({ turns: ['{"customer":"x","orders":0}'] })
    const r = await runHarnessStreamed(harness([FIND]), { email: 'e' }, { caller: 'test:loop', deps: s.deps }, { stream: async () => ({ kind: 'gateway', text: '', toolNames: [], usage: null, contractDropped: false }) })
    expect(r.value).toBeNull()
    expect(r.error).toMatch(/a tool loop does not stream/)
    expect(s.requests).toHaveLength(0)
  })

  it('leaves a run with no loop exactly as it was: no loop fields on the result', async () => {
    const plain = { ...harness([FIND]), loop: undefined }
    const r = await run(plain, scripted({ turns: ['{"customer":"x","orders":0}'] }).deps)
    expect(r.value).toEqual({ customer: 'x', orders: 0 })
    expect('calls' in r).toBe(false)
    expect('toolCalls' in r).toBe(false)
  })
})
