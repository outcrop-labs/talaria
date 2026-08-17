// "Refresh tools" failed on BOTH of the servers Talaria ships, for two
// unrelated reasons — and both failed in a way that told the operator the
// platform's own tools were unreachable.
//
//   Workbench   `talaria-workbench://core` is a ROUTING TOKEN, not an endpoint.
//               Its catalog is a module export and always has been (that is how
//               the row is seeded), but refresh went down the HTTP path and
//               tried to fetch a URL with a scheme no client resolves.
//   Toolkit     runs as a child process spawned OPPORTUNISTICALLY — on renders
//               and comms reads. On a freshly booted app neither has happened,
//               so the port is genuinely empty and the probe was right to fail.
//               Refresh has to start it, not report it dead.
//
// What is pinned here is the ROUTING: which of the three paths a server takes,
// and specifically that neither internal server reaches the network.
import { describe, expect, it, vi, beforeEach } from 'vitest'

const rows: Record<string, unknown>[] = []
const sql = Object.assign(() => Promise.resolve(rows), {
  unsafe: () => Promise.resolve(rows),
  json: (v: unknown) => v,
}) as never
vi.mock('@/server/db/pg', () => ({ db: async () => sql }))

// If either internal path reaches for the network, these fire and the test
// fails — which is the whole assertion.
const safeFetch = vi.fn(() => Promise.reject(new Error('safeFetch must not be called for an internal server')))
vi.mock('@/server/safe-fetch', () => ({ safeFetch: (...a: unknown[]) => safeFetch(...(a as [])), BlockedUrlError: class extends Error {} }))

const awaitMcpService = vi.fn(() => Promise.resolve(true))
vi.mock('@/server/mcp-service', () => ({
  awaitMcpService: () => awaitMcpService(),
  MCP_PORT: () => 5280,
  ensureMcpService: () => {},
}))

vi.mock('@/server/workbench-mcp', () => ({
  WORKBENCH_TOOLS: [
    { name: 'start_job', description: 'Start a sandboxed job' },
    { name: 'open_pr', description: 'Open a pull request' },
  ],
}))
vi.mock('@/server/secretbox', () => ({ seal: (v: string) => v, open: (v: string) => v }))
vi.mock('@/server/users', () => ({ assistantOwnerFor: async () => null }))
vi.mock('@/server/agent-auth', () => ({ subjectModel: () => null }))
vi.mock('@/server/mcp-oauth', () => ({ hasOauthTokens: async () => false, oauthTokenFor: async () => null }))

const { refreshMcpTools } = await import('@/server/mcp-registry')

const setServer = (over: Record<string, unknown>) => {
  rows.length = 0
  rows.push({
    id: 'srv-1',
    name: 'x',
    label: 'X',
    url: 'https://mcp.example.com/mcp',
    headers: {},
    timeoutSecs: null,
    enabled: true,
    allAgents: false,
    builtin: false,
    appSlug: null,
    oauthEnabled: false,
    tools: [],
    ...over,
  })
}

beforeEach(() => {
  safeFetch.mockClear()
  awaitMcpService.mockClear()
  awaitMcpService.mockResolvedValue(true)
})

describe('refreshMcpTools — the Workbench', () => {
  it('reads its catalog in-process and never touches the network', async () => {
    setServer({ name: 'workbench', url: 'talaria-workbench://core' })
    const out = await refreshMcpTools('srv-1')
    expect(out).toEqual({ tools: [
      { name: 'start_job', description: 'Start a sandboxed job' },
      { name: 'open_pr', description: 'Open a pull request' },
    ] })
    expect(safeFetch).not.toHaveBeenCalled()
  })

  it('does not go looking for the toolkit child process either', async () => {
    setServer({ name: 'workbench', url: 'talaria-workbench://core' })
    await refreshMcpTools('srv-1')
    expect(awaitMcpService).not.toHaveBeenCalled()
  })
})

describe('refreshMcpTools — the built-in toolkit', () => {
  it('starts the service before probing it', async () => {
    setServer({ name: 'talaria', builtin: true, url: 'http://127.0.0.1:5280/mcp' })
    await refreshMcpTools('srv-1').catch(() => {})
    expect(awaitMcpService).toHaveBeenCalled()
  })

  it('says the service would not start, rather than blaming the endpoint', async () => {
    awaitMcpService.mockResolvedValue(false)
    setServer({ name: 'talaria', builtin: true, url: 'http://127.0.0.1:5280/mcp' })
    const out = await refreshMcpTools('srv-1')
    expect(out).toHaveProperty('error')
    expect((out as { error: string }).error).toMatch(/did not start/)
    // It must not have gone on to probe a port it knows is empty.
    expect(safeFetch).not.toHaveBeenCalled()
  })
})

describe('refreshMcpTools — an external server', () => {
  it('is unaffected: no in-process shortcut, no service spawn', async () => {
    setServer({ url: 'https://mcp.linear.app/mcp' })
    await refreshMcpTools('srv-1').catch(() => {})
    expect(awaitMcpService).not.toHaveBeenCalled()
    // It DID reach for the network — that is the correct path for this one.
    expect(safeFetch).toHaveBeenCalled()
  })
})
