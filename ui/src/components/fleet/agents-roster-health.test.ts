import { describe, expect, it, vi } from 'vitest'
import type { AgentContainers, AgentDef } from '@/lib/fleet-defs'

// The draft mapping (TALA-11): the module's own reactive hooks
// (useAgentControls/useAgentMenu) pull svelte-query and the session, neither
// of which belongs in a node test — the seam under test is healthOf, a pure
// function of (def, containers). The peers are mocked away so the import
// resolves; nothing in this suite calls them.
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: vi.fn() }))
vi.mock('@/lib/session', () => ({ useSession: vi.fn() }))

// Lives HERE and not beside the module it tests: `src/routes/**` is
// file-based routing, where a dot is a path separator — a colocated
// agents.test.ts would be a route handler, which is why vitest excludes the
// whole directory (see vitest.config.ts). The module under test is
// routes/app/agents.svelte.ts; its consumers (AgentItem) live one door down
// from this file.
import { HEALTH_COLOR, healthOf } from '@/routes/app/agents.svelte'

const def = (over: Partial<AgentDef> = {}): AgentDef =>
  ({
    id: 'a1',
    slug: 'analyst',
    department: 'research',
    displayName: 'Analyst',
    enabled: true,
    managed: true,
    source: 'created',
    currentVersion: 1,
    ...over,
  }) as AgentDef

const containers = (managed: AgentContainers['managed']): AgentContainers => ({ department: 'research', managed })

const RUNNING = containers({ name: 'x', state: 'running', status: 'Up', health: 'healthy' })

describe('healthOf: the draft state', () => {
  it('reads a start=false hire (created, no container at all) as draft, not down', () => {
    expect(healthOf(def(), null)).toEqual({ health: 'draft', running: false })
    expect(healthOf(def(), containers(null))).toEqual({ health: 'draft', running: false })
  })

  it('never drafts an agent whose container exists — a live container beats the hire intent', () => {
    expect(healthOf(def(), RUNNING).health).toBe('up')
    // Even a created/exited container means the boot happened at some point:
    // that is 'down' (something to look at), not a draft.
    const exited = containers({ name: 'x', state: 'exited', status: 'Exited', health: null })
    expect(healthOf(def(), exited)).toEqual({ health: 'down', running: false })
  })

  it('never drafts imported or unmanaged defs — only Talaria-hired ones', () => {
    // Imported without a container was never ours to boot: stays down.
    expect(healthOf(def({ source: 'imported' }), null).health).toBe('down')
    // Unmanaged likewise.
    expect(healthOf(def({ managed: false }), null).health).toBe('down')
  })

  it('never drafts a retired def — disabled wins first, as before', () => {
    expect(healthOf(def({ enabled: false }), null)).toEqual({ health: 'retired', running: false })
  })

  it('carries a neutral color, not a signal tone — a draft is not a fault', () => {
    expect(HEALTH_COLOR.draft).toBe('var(--theme-ink-dim)')
    // Distinct from the colors that mean something is wrong or something is
    // live: retired is the line token, down the danger one — draft is neither
    // signal, it is a state word.
    expect(HEALTH_COLOR.draft).not.toBe(HEALTH_COLOR.down)
    expect(HEALTH_COLOR.draft).not.toBe(HEALTH_COLOR.retired)
    expect(HEALTH_COLOR.draft).not.toBe(HEALTH_COLOR.up)
  })
})

describe('healthOf: the five original states are unchanged', () => {
  it('up / warming / degraded follow the running container', () => {
    expect(healthOf(def(), RUNNING).health).toBe('up')
    const warming = containers({ name: 'x', state: 'running', status: 'Up', health: 'starting' })
    expect(healthOf(def(), warming)).toEqual({ health: 'warming', running: true })
    const sick = containers({ name: 'x', state: 'running', status: 'Up', health: 'unhealthy' })
    expect(healthOf(def(), sick)).toEqual({ health: 'degraded', running: true })
  })

  it('down and retired keep their meaning', () => {
    const stopped = containers({ name: 'x', state: 'exited', status: 'Exited', health: null })
    expect(healthOf(def({ source: 'imported' }), stopped).health).toBe('down')
    expect(healthOf(def({ enabled: false }), stopped).health).toBe('retired')
  })
})
