import { beforeEach, describe, expect, it, vi } from 'vitest'

// THE QUESTION THIS FILE PINS DOWN: the TS resolver is a twin of the Rust
// `effective_mcp_for` (api/crates/talaria-mcp/src/registry.rs), and the drift
// it had was that team grants worked for every MCP server except the
// app-published ones this resident serves. The tables are faked by query
// shape — the module's SQL names its tables, and that is what a row set is
// keyed on here — so each case states which rows exist and what the verdict
// must be, with the Rust semantics (null = all tools, [] = nothing).

type Grant = { allowed: boolean; tools: string[] | null }
const state = vi.hoisted(() => ({
  server: null as Record<string, unknown> | null,
  agentRows: [] as Array<{ tools: string[] | null }>,
  teamAgentRows: [] as Grant[],
  userAccess: [] as Grant[],
  teamUserRows: [] as Grant[],
  owner: null as string | null,
}))

vi.mock('./db/pg', () => {
  const tagged = async (strings: TemplateStringsArray) => {
    const q = strings.join('?')
    if (q.includes('from mcp_server_agents')) return state.agentRows
    if (q.includes('join team_agents')) return state.teamAgentRows
    if (q.includes('from mcp_user_access')) return state.userAccess
    if (q.includes('join team_members')) return state.teamUserRows
    throw new Error(`unexpected query: ${q}`)
  }
  const sql = Object.assign(tagged, {
    unsafe: async (q: string) => (q.includes('from mcp_servers') && state.server ? [state.server] : []),
  })
  return { db: async () => sql }
})
vi.mock('./users', () => ({ assistantOwnerFor: async () => state.owner }))
vi.mock('./agent-auth', () => ({
  subjectModel: (s: unknown) => (typeof s === 'string' ? s : (s as { model: string }).model),
  subjectProven: (s: unknown) => typeof s === 'string' || !(s as { legacy: boolean }).legacy,
}))
vi.mock('./mcp-oauth', () => ({ oauthTokenFor: async () => null }))
vi.mock('./secretbox', () => ({ open: (s: string) => s }))
vi.mock('./safe-fetch', () => ({ safeFetch: async () => { throw new Error('no network in this test') } }))

import { effectiveMcpFor } from './mcp-registry'

const server = (over: Record<string, unknown> = {}) => ({
  id: 'srv-1',
  name: 'app-crm',
  label: 'CRM',
  description: null,
  url: 'talaria-app://crm',
  headers: {},
  timeoutSecs: null,
  enabled: true,
  allAgents: false,
  authMode: 'org',
  tools: [],
  toolsRefreshedAt: null,
  requiredHeaders: [],
  oauthEnabled: false,
  builtin: false,
  appSlug: 'crm',
  createdBy: null,
  createdAt: '2026-10-20T00:00:00.000Z',
  ...over,
})

beforeEach(() => {
  state.server = server()
  state.agentRows = []
  state.teamAgentRows = []
  state.userAccess = []
  state.teamUserRows = []
  state.owner = null
})

const tools = async () => (await effectiveMcpFor('sales-bot', 'app-crm'))?.tools

describe('effectiveMcpFor — the agent side', () => {
  it('a scoped server with no row and no team grant is denied', async () => {
    expect(await effectiveMcpFor('sales-bot', 'app-crm')).toBeNull()
  })

  it('a team grant counts as assignment on a scoped server (the drifted case)', async () => {
    // Admin granted app-crm to the "sales" team; the agent is on it. This
    // answered 403 before, while the same grant on a remote server worked.
    state.teamAgentRows = [{ allowed: true, tools: ['contacts_search'] }]
    expect(await tools()).toEqual(['contacts_search'])
  })

  it('a team DENY row grants nothing on its own', async () => {
    state.teamAgentRows = [{ allowed: false, tools: null }]
    expect(await effectiveMcpFor('sales-bot', 'app-crm')).toBeNull()
  })

  it('assignment ∪ team grants, with null meaning all', async () => {
    state.agentRows = [{ tools: ['a'] }]
    state.teamAgentRows = [{ allowed: true, tools: ['b'] }, { allowed: true, tools: ['a', 'c'] }]
    expect(await tools()).toEqual(['a', 'b', 'c'])
    state.teamAgentRows = [{ allowed: true, tools: null }]
    expect(await tools()).toBeNull() // a team grant to all tools dominates
  })

  it('an all-agents server carries everyone; a row only narrows', async () => {
    state.server = server({ allAgents: true })
    expect(await tools()).toBeNull()
    state.agentRows = [{ tools: ['x'] }]
    expect(await tools()).toEqual(['x'])
  })

  it('a disabled or unknown server is denied', async () => {
    state.server = server({ enabled: false })
    expect(await effectiveMcpFor('sales-bot', 'app-crm')).toBeNull()
    state.server = null
    expect(await effectiveMcpFor('sales-bot', 'app-crm')).toBeNull()
  })

  it('the workbench is never resolved here', async () => {
    state.server = server({ name: 'workbench', allAgents: true, appSlug: null })
    expect(await effectiveMcpFor('sales-bot', 'workbench')).toBeNull()
  })
})

describe('effectiveMcpFor — the owner side (personal assistants)', () => {
  beforeEach(() => {
    state.owner = 'user-1'
    state.agentRows = [{ tools: null }]
  })

  it("the person's own deny wins outright, whatever the teams say", async () => {
    state.userAccess = [{ allowed: false, tools: null }]
    state.teamUserRows = [{ allowed: true, tools: null }]
    expect(await effectiveMcpFor('sales-bot', 'app-crm')).toBeNull()
  })

  it('a team deny with no allow anywhere denies', async () => {
    state.teamUserRows = [{ allowed: false, tools: null }]
    expect(await effectiveMcpFor('sales-bot', 'app-crm')).toBeNull()
  })

  it("a team deny is overridden by the person's own allow", async () => {
    state.userAccess = [{ allowed: true, tools: ['contacts_search'] }]
    state.teamUserRows = [{ allowed: false, tools: null }]
    expect(await tools()).toEqual(['contacts_search'])
  })

  it('a team deny is overridden by another team allow', async () => {
    state.teamUserRows = [{ allowed: false, tools: null }, { allowed: true, tools: ['a'] }]
    expect(await tools()).toEqual(['a'])
  })

  it("the owner's allowance (person ∪ teams) intersects the agent's", async () => {
    state.agentRows = [{ tools: ['a', 'b', 'c'] }]
    state.userAccess = [{ allowed: true, tools: ['a'] }]
    state.teamUserRows = [{ allowed: true, tools: ['c', 'z'] }]
    expect(await tools()).toEqual(['a', 'c'])
  })

  it('no person row and no team rows is no narrowing', async () => {
    state.agentRows = [{ tools: ['a'] }]
    expect(await tools()).toEqual(['a'])
  })
})
