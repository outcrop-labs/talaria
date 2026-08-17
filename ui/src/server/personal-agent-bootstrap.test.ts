// A FRESH INSTALL MUST BE ABLE TO MAKE THE FIRST AGENT.
//
// Creating a personal assistant required an existing enabled agent to clone,
// and said so: "no agent to base a personal assistant on — import a stack
// first". That failed precisely the case it most needed to serve — a new
// deployment, where the first thing someone asks for is their own assistant,
// and where there is nothing to import because Talaria IS the stack.
//
// The existing agent was only ever a CHASSIS shortcut (model tiers, tools,
// plugins). The assistant's identity — soul, role, department — is written by
// this module either way, and `createAgent` has always fallen back to platform
// defaults when handed no template.
import { describe, expect, it, vi, beforeEach } from 'vitest'

/** Rows the CHASSIS lookup returns (`select id from agent_defs where enabled`). */
let agentRows: Array<{ id: string }> = []

// Query-aware, because two different `agent_defs` reads run here and they must
// not answer each other: the owner lookup decides whether an assistant already
// exists (it must not), the enabled lookup supplies the chassis shortcut.
const sql = Object.assign(
  (strings: TemplateStringsArray) => {
    const text = strings.join(' ').replace(/\s+/g, ' ').toLowerCase()
    if (text.includes('owner_user_id =')) return Promise.resolve([])
    if (text.includes('select id from agent_defs where enabled')) return Promise.resolve(agentRows)
    return Promise.resolve([])
  },
  { unsafe: () => Promise.resolve([]), json: (v: unknown) => v },
) as never
vi.mock('@/server/db/pg', () => ({ db: async () => sql }))

type CreateArg = Record<string, unknown>
const createAgent = vi.fn(async (_input: CreateArg) => ({ def: { id: 'new-1', model: 'assistant-personal', department: 'personal' }, keyCreated: true }))
vi.mock('@/server/fleet-create', () => ({
  createAgent: (input: CreateArg) => createAgent(input),
  setAgentEnabled: vi.fn(async () => {}),
}))

vi.mock('@/server/fleet-render', () => ({ renderFleet: async () => {}, FLEET_DIR: () => '/tmp' }))
vi.mock('@/server/fleet-docker', () => ({ fleetUp: async () => {}, waitHealthy: async () => {} }))
vi.mock('@/server/retrieval/collections', () => ({ ensurePersonalCollection: async () => {} }))
vi.mock('@/server/kb', () => ({ syncUserPrivateDocs: async () => {} }))

const mod = await import('@/server/personal-agent')

const user = { id: 'u1', email: 'jon@example.com', name: 'Jon Iler' }

beforeEach(() => {
  createAgent.mockClear()
  agentRows = []
})

describe('creating a personal assistant on a fresh install', () => {
  it('does not demand an existing agent', async () => {
    // The regression: this used to throw before reaching createAgent at all.
    await mod.createPersonalAgent(user).catch(() => {})
    expect(createAgent).toHaveBeenCalled()
  })

  it('asks for platform defaults rather than naming a template', async () => {
    await mod.createPersonalAgent(user).catch(() => {})
    const arg = createAgent.mock.calls[0]?.[0]
    expect(arg).toBeDefined()
    expect(arg).not.toHaveProperty('templateId')
  })

  it('still writes the assistant’s own identity, which never came from a template', async () => {
    await mod.createPersonalAgent(user).catch(() => {})
    const arg = createAgent.mock.calls[0]![0]
    expect(arg.role).toBe('Personal assistant')
    expect(String(arg.soul ?? '')).not.toHaveLength(0)
    expect(String(arg.displayName ?? '')).toContain('Jon')
  })
})

describe('when the fleet already has an agent', () => {
  it('still clones it as a chassis shortcut', async () => {
    agentRows = [{ id: 'existing-agent' }]
    await mod.createPersonalAgent(user).catch(() => {})
    const arg = createAgent.mock.calls[0]![0]
    expect(arg.templateId).toBe('existing-agent')
  })
})
