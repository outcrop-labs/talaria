// THE DATABASE SAYS WHICH AGENTS EXIST. The manifest only says where to reach
// them.
//
// `fleet/fleet.json` is a render OUTPUT, and it was being read as a roster. On
// a machine whose database had been replaced but whose working tree had not,
// the app listed six agents that no longer existed anywhere — names, roles and
// all — because the stale file on disk was the only thing anyone asked.
//
// The `defs.length === 0` early return is what made it possible. It was meant
// as a bootstrap convenience ("the DB isn't populated yet, don't filter"), but
// an empty definition table does not mean "unknown", it means NONE — and that
// is the one case where returning the whole manifest is exactly wrong.
import { describe, expect, it, vi, beforeEach } from 'vitest'

let defs: Array<{ model: string; config: { aliases?: Array<{ name: string }> } | null }> = []

const sql = (() => Promise.resolve(defs)) as never
vi.mock('@/server/db/pg', () => ({ db: async () => sql }))

const listAgents = vi.fn()
vi.mock('@/server/gateway', () => ({ listAgents: () => listAgents() }))

const { listFleetAgents, routedModelFor } = await import('@/server/fleet-agents')

/** What a rendered manifest looks like once tier entries are dropped. */
const manifest = (...ids: string[]) => ids.map((id) => ({ id, label: id, role: '' }))

beforeEach(() => {
  defs = []
  listAgents.mockReset()
})

describe('listFleetAgents', () => {
  it('returns nothing when the database has no agents, however full the manifest is', async () => {
    // The exact shape of the bug: a manifest left behind by a previous install.
    listAgents.mockResolvedValue(manifest('engineer-engineering', 'analyst-research', 'support-support'))
    defs = []
    expect(await listFleetAgents()).toEqual([])
  })

  it('returns nothing when both are empty', async () => {
    listAgents.mockResolvedValue([])
    expect(await listFleetAgents()).toEqual([])
  })

  it('returns only the agents the database claims', async () => {
    listAgents.mockResolvedValue(manifest('engineer-engineering', 'ghost-department'))
    defs = [{ model: 'engineer-engineering', config: null }]
    const out = await listFleetAgents()
    expect(out.map((a) => a.id)).toEqual(['engineer-engineering'])
  })

  it('drops a defined agent that is not in the manifest — it has nowhere to be reached', async () => {
    listAgents.mockResolvedValue(manifest('engineer-engineering'))
    defs = [
      { model: 'engineer-engineering', config: null },
      { model: 'analyst-research', config: null }, // defined but never rendered
    ]
    expect((await listFleetAgents()).map((a) => a.id)).toEqual(['engineer-engineering'])
  })

  it('carries the tiers declared on the definition', async () => {
    listAgents.mockResolvedValue(manifest('engineer-engineering'))
    defs = [{ model: 'engineer-engineering', config: { aliases: [{ name: 'opus' }, { name: 'haiku' }] } }]
    expect((await listFleetAgents())[0]?.tiers).toEqual(['opus', 'haiku'])
  })
})

describe('routedModelFor', () => {
  it('refuses a tier for an agent the database does not have', async () => {
    listAgents.mockResolvedValue(manifest('engineer-engineering'))
    defs = []
    expect(await routedModelFor('engineer-engineering', 'opus')).toBeNull()
  })

  it('refuses a tier the agent does not declare', async () => {
    listAgents.mockResolvedValue(manifest('engineer-engineering'))
    defs = [{ model: 'engineer-engineering', config: { aliases: [{ name: 'opus' }] } }]
    expect(await routedModelFor('engineer-engineering', 'haiku')).toBeNull()
  })

  it('routes a declared tier', async () => {
    listAgents.mockResolvedValue(manifest('engineer-engineering'))
    defs = [{ model: 'engineer-engineering', config: { aliases: [{ name: 'opus' }] } }]
    expect(await routedModelFor('engineer-engineering', 'opus')).toBe('engineer-engineering-opus')
  })

  it('passes an untiered request through untouched', async () => {
    expect(await routedModelFor('engineer-engineering', null)).toBe('engineer-engineering')
  })
})
