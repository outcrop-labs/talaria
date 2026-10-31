import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest'

// THE QUESTION THIS FILE PINS DOWN: what the app MCP gateway route
// (/api/mcp/gw/app-<slug>, routes/api/mcp.gw.$server.ts) does for an app that
// is not serving. Turning an app off takes its server off every agent's
// rendered config, but a running agent keeps the server until its next
// reconcile of that config, so the route itself refuses: a 403 with the
// reason, recorded like any refused call, and the app's module is never
// loaded. The route lives under routes/, which vitest does not collect, so it
// is driven from here.

const st = vi.hoisted(() => ({
  enabled: [] as string[],
  appOff: null as string | null,
  records: [] as Array<Record<string, unknown>>,
  loads: 0,
  caller: { id: 'def-1', model: 'crm-agent', legacy: false } as { id: string | null; model: string; legacy: boolean },
  subset: null as string[] | null,
  actingUserId: null as string | null,
  timeoutSecs: null as number | null,
  extraTools: [] as unknown[],
}))

vi.mock('@/server/agent-auth', () => ({
  requireAgent: async () => st.caller,
}))
vi.mock('@/server/mcp-registry', () => ({
  // Granted, every tool unless a test narrows it: the refusals under test are
  // the app's and the wire's, not the grant's.
  effectiveMcpFor: async () => ({
    server: { id: 'srv-1', name: 'app-crm', appSlug: 'crm', appOff: st.appOff, enabled: true, timeoutSecs: st.timeoutSecs },
    tools: st.subset,
    actingUserId: st.actingUserId,
  }),
  recordMcpToolCall: (row: Record<string, unknown>) => {
    st.records.push(row)
  },
}))
vi.mock('@/server/audit', () => ({
  getSetting: async (key: string, fallback: unknown) => (key === 'apps_enabled' ? st.enabled : fallback),
}))
const tools = [{ name: 'crm_find', description: 'Find a contact.', inputSchema: { type: 'object' }, handler: async () => 'found' }]
vi.mock('@/server/app-load', () => ({
  appHasMcp: () => true,
  loadAppMcp: async () => {
    st.loads++
    return { default: { tools: [...tools, ...st.extraTools] } }
  },
}))
vi.mock('@/server/app-store', () => ({ storeFor: () => ({}) }))
vi.mock('@/server/internal-api', () => ({ resyncAppMcpServers: async () => [] }))
vi.mock('@/server/workspace-secrets', () => ({
  spendHandlesInToolCall: async () => ({ used: [], unresolved: [] }),
}))
// The catalog the first dispatch writes lands in a scratch data directory.
const dataDir = mkdtempSync(join(tmpdir(), 'app-mcp-gw-test-'))
process.env.TALARIA_APP_DATA_DIR = dataDir
afterAll(() => rmSync(dataDir, { recursive: true, force: true }))

const { Route } = await import('@/routes/api/mcp.gw.$server')

const post = (body: BodyInit, signal?: AbortSignal) =>
  Route.handlers.POST!({
    request: new Request('http://talaria.test/api/mcp/gw/app-crm', {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: 'Bearer tak_x' },
      body,
      signal,
    }),
    params: { server: 'app-crm' },
  })
const call = (method: string, params?: Record<string, unknown>, signal?: AbortSignal) =>
  post(JSON.stringify({ jsonrpc: '2.0', id: 7, method, params }), signal)

beforeEach(() => {
  st.enabled = []
  st.appOff = null
  st.records = []
  st.loads = 0
  st.caller = { id: 'def-1', model: 'crm-agent', legacy: false }
  st.subset = null
  st.actingUserId = null
  st.timeoutSecs = null
  st.extraTools = []
})

describe('the app MCP gateway route', () => {
  it('refuses every tool of an app that is not enabled, records the refusal, and never loads the module', async () => {
    const res = await call('tools/call', { name: 'crm_find', arguments: { q: 'ada' } })
    expect(res.status).toBe(403)
    expect(await res.json()).toEqual({ error: 'app "crm" is not enabled' })
    expect(st.records).toEqual([
      expect.objectContaining({
        serverId: 'srv-1',
        serverName: 'app-crm',
        tool: 'crm_find',
        outcome: 'denied',
        status: 403,
        error: 'app "crm" is not enabled',
      }),
    ])
    expect(st.loads).toBe(0)
  })

  it('refuses a listing too; only calls leave a record', async () => {
    const res = await call('tools/list')
    expect(res.status).toBe(403)
    expect(st.records).toEqual([])
    expect(st.loads).toBe(0)
  })

  it('refuses an enabled app whose row the api turned off, with the api’s reason', async () => {
    st.enabled = ['crm']
    st.appOff = 'the app no longer publishes MCP tools'
    const res = await call('tools/call', { name: 'crm_find', arguments: {} })
    expect(res.status).toBe(403)
    const off = 'app "crm" is not serving: the app no longer publishes MCP tools'
    expect(await res.json()).toEqual({ error: off })
    expect(st.records).toEqual([expect.objectContaining({ outcome: 'denied', status: 403, error: off })])
    expect(st.loads).toBe(0)
  })

  it('serves a serving app, and records the call', async () => {
    st.enabled = ['crm']
    const res = await call('tools/call', { name: 'crm_find', arguments: { q: 'ada' } })
    expect(res.status).toBe(200)
    const body = (await res.json()) as { result: { content: Array<{ text: string }> } }
    expect(body.result.content[0]!.text).toBe('found')
    expect(st.records).toEqual([expect.objectContaining({ tool: 'crm_find', outcome: 'ok', status: 200 })])
  })

  it('refuses the notification stream of an app that is not enabled', async () => {
    const res = await Route.handlers.GET!({
      request: new Request('http://talaria.test/api/mcp/gw/app-crm', { headers: { authorization: 'Bearer tak_x' } }),
      params: { server: 'app-crm' },
    })
    expect(res.status).toBe(403)
    expect(await res.json()).toEqual({ error: 'app "crm" is not enabled' })
  })
})

// ── The handler's context: who, for whom, until when ────────────────────────

/** A tool that answers its own context, minus the store. */
const whoami = {
  name: 'crm_whoami',
  description: 'Echo the call context.',
  handler: async (_args: unknown, ctx: { agent: string; caller: unknown; actingUserId: unknown; app: string; signal: AbortSignal }) =>
    ({ app: ctx.app, agent: ctx.agent, caller: ctx.caller, actingUserId: ctx.actingUserId, aborted: ctx.signal.aborted }),
}
const body = async (res: Response) => (await res.json()) as { result?: { content: Array<{ text: string }>; isError?: boolean; structuredContent?: unknown }; error?: { code: number; message: string } }

describe('the handler context', () => {
  beforeEach(() => {
    st.enabled = ['crm']
    st.extraTools = [whoami]
  })

  it('a call on the agent’s own key is a proven caller, with the person it acts for', async () => {
    st.actingUserId = 'user-9'
    const r = await body(await call('tools/call', { name: 'crm_whoami', arguments: {} }))
    expect(JSON.parse(r.result!.content[0]!.text)).toEqual({
      app: 'crm',
      agent: 'crm-agent',
      caller: { name: 'crm-agent', id: 'def-1', proven: true },
      actingUserId: 'user-9',
      aborted: false,
    })
  })

  it('a call on the org-wide key names its agent but proves nothing', async () => {
    st.caller = { id: null, model: 'claimed-agent', legacy: true }
    const r = await body(await call('tools/call', { name: 'crm_whoami', arguments: {} }))
    expect(JSON.parse(r.result!.content[0]!.text)).toMatchObject({
      agent: 'claimed-agent',
      caller: { name: 'claimed-agent', id: null, proven: false },
      actingUserId: null,
    })
  })

  it('a slow tool is cut at the server’s timeout: the handler sees the abort, the agent an error, the record says why', async () => {
    st.timeoutSecs = 1
    let sawAbort: string | null = null
    st.extraTools = [
      {
        name: 'crm_slow',
        description: 'Takes its time.',
        handler: (_a: unknown, ctx: { signal: AbortSignal }) =>
          new Promise((resolve) => {
            ctx.signal.addEventListener('abort', () => {
              sawAbort = (ctx.signal.reason as Error).name
            })
            setTimeout(() => resolve('too late'), 5_000)
          }),
      },
    ]
    const started = Date.now()
    const res = await call('tools/call', { name: 'crm_slow', arguments: {} })
    expect(Date.now() - started).toBeLessThan(3_000)
    const r = await body(res)
    expect(r.result!.isError).toBe(true)
    expect(r.result!.content[0]!.text).toContain('did not finish within 1 s')
    expect(sawAbort).toBe('TimeoutError')
    expect(st.records).toEqual([
      expect.objectContaining({
        tool: 'crm_slow',
        outcome: 'error',
        status: 200,
        error: "the tool did not finish within 1 s (the server's timeout); the call was cut off",
      }),
    ])
  })

  it('an agent that disconnects cuts the call; it is recorded cancelled, not failed', async () => {
    const gone = new AbortController()
    let sawAbort = false
    st.extraTools = [
      {
        name: 'crm_slow',
        description: 'Takes its time.',
        handler: (_a: unknown, ctx: { signal: AbortSignal }) =>
          new Promise((resolve) => {
            ctx.signal.addEventListener('abort', () => (sawAbort = true))
            setTimeout(() => resolve('too late'), 5_000)
          }),
      },
    ]
    setTimeout(() => gone.abort(), 50)
    const res = await call('tools/call', { name: 'crm_slow', arguments: {} }, gone.signal)
    expect(res.status).toBe(499)
    expect(sawAbort).toBe(true)
    expect(st.records).toEqual([
      expect.objectContaining({ tool: 'crm_slow', outcome: 'cancelled', status: 499, error: 'the caller disconnected before the answer finished' }),
    ])
  })

  it('a structured result reaches the agent unchanged, checked against the outputSchema', async () => {
    st.extraTools = [
      {
        name: 'crm_count',
        description: 'Count contacts.',
        outputSchema: { type: 'object', properties: { count: { type: 'number' } }, required: ['count'] },
        handler: async (args: { bad?: boolean }) => (args.bad ? { count: 'many' } : { count: 3 }),
      },
    ]
    const ok = await body(await call('tools/call', { name: 'crm_count', arguments: {} }))
    expect(ok.result).toEqual({ content: [{ type: 'text', text: '{"count":3}' }], structuredContent: { count: 3 } })
    const list = await body(await call('tools/list'))
    expect((list.result as unknown as { tools: Array<{ name: string; outputSchema?: unknown }> }).tools.find((t) => t.name === 'crm_count')?.outputSchema).toEqual({
      type: 'object',
      properties: { count: { type: 'number' } },
      required: ['count'],
    })
    const bad = await body(await call('tools/call', { name: 'crm_count', arguments: { bad: true } }))
    expect(bad.error?.code).toBe(-32603)
    expect(bad.error?.message).toContain('tool "crm_count" returned structuredContent its outputSchema refuses')
    expect(st.records.at(-1)).toMatchObject({ tool: 'crm_count', outcome: 'error', error: expect.stringContaining('outputSchema refuses') })
  })
})

// ── The wire: the Rust gateway's answers, from this one too ────────────────

describe('a body the gateway will not dispatch', () => {
  beforeEach(() => {
    st.enabled = ['crm']
  })

  it('malformed JSON is a -32700 with the Rust sentence and a bounded hexdump, never dispatched', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const raw = '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"crm_find","arguments":{"q":"a\nb"}}}'
    const res = await post(raw)
    expect(res.status).toBe(200)
    const r = await body(res)
    expect(r.error?.code).toBe(-32700)
    expect((r as { id?: unknown }).id).toBeNull()
    expect(r.error?.message).toContain('request body is not valid JSON: control character')
    expect(r.error?.message).toContain('the fix is on the sender')
    expect(r.error?.message).toContain(' 0a ')
    expect(st.loads).toBe(0)
    expect(st.records).toEqual([])
    warn.mockRestore()
  })

  it('a batch is a -32600 per member, and each tools/call in it is recorded refused', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const res = await post(
      JSON.stringify([
        { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'crm_find', arguments: {} } },
        { jsonrpc: '2.0', method: 'notifications/initialized' },
        { jsonrpc: '2.0', id: 'b', method: 'tools/list' },
      ]),
    )
    const r = (await res.json()) as Array<{ id: unknown; error: { code: number; message: string } }>
    expect(r.map((m) => [m.id, m.error.code])).toEqual([
      [1, -32600],
      ['b', -32600],
    ])
    expect(r[0]!.error.message).toBe('a JSON-RPC batch carrying tools/call or tools/list is not accepted here; send each request on its own')
    expect(st.records).toEqual([expect.objectContaining({ tool: 'crm_find', outcome: 'denied', status: 200, error: r[0]!.error.message })])
    expect(st.loads).toBe(0)
    warn.mockRestore()
  })

  it('a body that is not a request object, or names no method, is a -32600', async () => {
    for (const raw of ['42', '"tools/call"', 'null', 'true']) {
      const r = await body(await post(raw))
      expect(r.error).toEqual({ code: -32600, message: 'the request is not a JSON-RPC request object' })
    }
    const r = await body(await post(JSON.stringify({ jsonrpc: '2.0', id: 4, params: {} })))
    expect(r.error).toEqual({ code: -32600, message: 'the request names no method' })
    expect((r as { id?: unknown }).id).toBe(4)
    expect(st.loads).toBe(0)
  })

  it('a leading BOM is stripped, so a well-formed body behind one is served', async () => {
    const json = new TextEncoder().encode(JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name: 'crm_find', arguments: {} } }))
    const res = await post(new Uint8Array([0xef, 0xbb, 0xbf, ...json]))
    expect((await body(res)).result!.content[0]!.text).toBe('found')
  })

  it('a tool subset closes resources, prompts and completion, recorded under the method', async () => {
    st.subset = ['crm_find']
    const r = await body(await call('resources/list', {}))
    expect(r.error).toEqual({ code: -32601, message: 'method "resources/list" is not available to this agent here (it holds a tool subset)' })
    expect(st.records).toEqual([expect.objectContaining({ method: 'resources/list', tool: null, outcome: 'denied', status: 200 })])
  })

  it('a refusal before resolution still names the tool the way the Rust record does', async () => {
    st.enabled = []
    await call('tools/call', { name: 42 })
    expect(st.records).toEqual([expect.objectContaining({ tool: null, outcome: 'denied' })])
  })
})
