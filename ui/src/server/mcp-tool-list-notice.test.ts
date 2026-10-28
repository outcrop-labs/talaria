import { beforeEach, describe, expect, it, vi } from 'vitest'

// The app gateway's tool-list notice: a keepalive from an agent behind the
// server's change count is answered with `notifications/tools/list_changed`
// ahead of the ping's result, once; a handshake starts the agent at the
// current count. Each server counts its own changes.

const st = vi.hoisted(() => ({ counts: new Map<string, number>(), store: new Map<string, string>() }))

vi.mock('@/server/audit', () => ({
  getSetting: async (key: string, fallback: unknown) =>
    key.startsWith('mcp.tool_list_count:') ? (st.counts.get(key.slice('mcp.tool_list_count:'.length)) ?? fallback) : fallback,
}))
vi.mock('@/server/db/redis', () => ({
  getRedis: () => ({
    get: async (k: string) => st.store.get(k) ?? null,
    set: async (k: string, v: string, ...opts: unknown[]) => {
      if (opts.includes('NX') && st.store.has(k)) return null
      st.store.set(k, v)
      return 'OK'
    },
  }),
}))

const { toolListNotice, toolListIsStale, toolsSeenKey, toolListCountKey } = await import('@/server/mcp-tool-list-notice')
const count = (server: string, n: number) => st.counts.set(server, n)

const req = (accept = 'application/json, text/event-stream') =>
  new Request('http://talaria.test/api/mcp/gw/app-crm', { method: 'POST', headers: { accept } })
const ping = { jsonrpc: '2.0', id: 9, method: 'ping' }
const initialize = { jsonrpc: '2.0', id: 1, method: 'initialize', params: {} }

async function events(resp: Response): Promise<unknown[]> {
  const text = await resp.text()
  return text
    .split('\n\n')
    .filter((e) => e.trim())
    .map((e) => {
      expect(e.startsWith('event: message\ndata: ')).toBe(true)
      return JSON.parse(e.slice('event: message\ndata: '.length))
    })
}

describe('the app gateway tool-list notice', () => {
  beforeEach(() => {
    st.counts.clear()
    st.store.clear()
  })

  it('tells a keepalive once, after a change since the handshake', async () => {
    count('app-crm', 3)
    expect(await toolListNotice('app-crm', 'crm-agent', initialize, req())).toBeNull()
    expect(st.store.get(toolsSeenKey('app-crm', 'crm-agent'))).toBe('3')
    // Nothing changed: the ping goes on to the app as always.
    expect(await toolListNotice('app-crm', 'crm-agent', ping, req())).toBeNull()

    count('app-crm', 4)
    const resp = await toolListNotice('app-crm', 'crm-agent', ping, req())
    expect(resp?.headers.get('content-type')).toBe('text/event-stream')
    expect(await events(resp!)).toEqual([
      { jsonrpc: '2.0', method: 'notifications/tools/list_changed' },
      { jsonrpc: '2.0', id: 9, result: {} },
    ])
    // Told once: the next keepalive is an ordinary one.
    expect(await toolListNotice('app-crm', 'crm-agent', ping, req())).toBeNull()
  })

  it('adopts the count for an agent it has never seen, and answers only SSE-taking pings', async () => {
    count('app-crm', 7)
    expect(await toolListNotice('app-crm', 'other-agent', ping, req())).toBeNull()
    expect(st.store.get(toolsSeenKey('app-crm', 'other-agent'))).toBe('7')
    count('app-crm', 8)
    expect(await toolListNotice('app-crm', 'other-agent', ping, req('application/json'))).toBeNull()
    expect(await toolListNotice('app-crm', 'other-agent', { jsonrpc: '2.0', id: 2, method: 'tools/list' }, req())).toBeNull()
  })

  it('keeps the count a reconnect finds, so a change before it is still told', async () => {
    count('app-crm', 2)
    expect(await toolListNotice('app-crm', 'crm-agent', initialize, req())).toBeNull()
    count('app-crm', 3)
    // A reconnect's handshake does not wipe what the agent last saw ...
    expect(await toolListNotice('app-crm', 'crm-agent', initialize, req())).toBeNull()
    expect(st.store.get(toolsSeenKey('app-crm', 'crm-agent'))).toBe('2')
    // ... so its next keepalive is told.
    expect(await toolListNotice('app-crm', 'crm-agent', ping, req())).not.toBeNull()
  })

  it("counts per server: another server's change tells this server's keepalive nothing", async () => {
    count('app-crm', 5)
    count('app-billing', 1)
    expect(await toolListNotice('app-crm', 'crm-agent', initialize, req())).toBeNull()
    expect(await toolListNotice('app-billing', 'crm-agent', initialize, req())).toBeNull()
    count('app-billing', 2)
    // The agent holds both; only the server that changed is re-listed.
    expect(await toolListNotice('app-crm', 'crm-agent', ping, req())).toBeNull()
    expect(await toolListNotice('app-billing', 'crm-agent', ping, req())).not.toBeNull()
    expect(toolListCountKey('app-crm')).toBe('mcp.tool_list_count:app-crm')
  })

  it('is stale only behind a remembered count', () => {
    expect(toolListIsStale(3, 4)).toBe(true)
    expect(toolListIsStale(4, 4)).toBe(false)
    expect(toolListIsStale(5, 4)).toBe(false)
    expect(toolListIsStale(null, 4)).toBe(false)
  })
})
