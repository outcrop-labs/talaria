import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeEach, describe, expect, it, vi } from 'vitest'

// THE QUESTION THIS FILE PINS DOWN: what the app MCP gateway route
// (/api/mcp/gw/app-<slug>, routes/api/mcp.gw.$server.ts) does for an app that
// is not serving. Turning an app off takes its server off every agent by a
// roll, but an agent's rendered config outlives that until the roll lands, so
// the route itself refuses: a 403 with the reason, recorded like any refused
// call, and the app's module is never loaded. The route lives under routes/,
// which vitest does not collect, so it is driven from here.

const st = vi.hoisted(() => ({
  enabled: [] as string[],
  appOff: null as string | null,
  records: [] as Array<Record<string, unknown>>,
  loads: 0,
}))

vi.mock('@/server/agent-auth', () => ({
  requireAgent: async () => ({ id: 'def-1', model: 'crm-agent', legacy: false }),
}))
vi.mock('@/server/mcp-registry', () => ({
  // Granted, every tool: the refusal under test is the app's, not the grant's.
  effectiveMcpFor: async () => ({
    server: { id: 'srv-1', name: 'app-crm', appSlug: 'crm', appOff: st.appOff, enabled: true },
    tools: null,
  }),
  recordMcpToolCall: (row: Record<string, unknown>) => {
    st.records.push(row)
  },
}))
vi.mock('@/server/audit', () => ({
  getSetting: async (key: string, fallback: unknown) => (key === 'apps_enabled' ? st.enabled : fallback),
}))
vi.mock('@/server/app-load', () => ({
  appHasMcp: () => true,
  loadAppMcp: async () => {
    st.loads++
    return {
      default: {
        tools: [{ name: 'crm_find', description: 'Find a contact.', inputSchema: { type: 'object' }, handler: async () => 'found' }],
      },
    }
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

const call = (method: string, params?: Record<string, unknown>) =>
  Route.handlers.POST!({
    request: new Request('http://talaria.test/api/mcp/gw/app-crm', {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: 'Bearer tak_x' },
      body: JSON.stringify({ jsonrpc: '2.0', id: 7, method, params }),
    }),
    params: { server: 'app-crm' },
  })

beforeEach(() => {
  st.enabled = []
  st.appOff = null
  st.records = []
  st.loads = 0
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
