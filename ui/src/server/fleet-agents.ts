// The fleet as AGENTS (not raw gateway models): the bridge's /v1/models now
// includes one entry per tier (`<base>-<alias>`), so consumers that mean
// "agents" must filter to definition models and pick tiers up separately.
import { db } from './db/pg'
import { listAgents, type AgentModel } from './gateway'
import type { AgentConfig } from './agent-defs'

export interface FleetAgentEntry extends AgentModel {
  /** Model tiers requestable for this agent (alias names; main is implicit). */
  tiers: string[]
}

export async function listFleetAgents(): Promise<{ agents: FleetAgentEntry[]; source: 'gateway' | 'mock' }> {
  const { agents, source } = await listAgents()
  const sql = await db()
  const defs = (await sql`
    select d.model, v.config
    from agent_defs d
    left join agent_versions v on v.agent_id = d.id and v.version = d.current_version
    where d.enabled
  `) as unknown as Array<{ model: string; config: AgentConfig | null }>
  if (defs.length === 0) return { agents: agents.map((a) => ({ ...a, tiers: [] })), source }
  const byModel = new Map(defs.map((d) => [d.model, d.config?.aliases?.map((a) => a.name) ?? []]))
  return {
    agents: agents.filter((a) => byModel.has(a.id)).map((a) => ({ ...a, tiers: byModel.get(a.id)! })),
    source,
  }
}

/** Validate a tier for an agent; returns the routed gateway model id. */
export async function routedModelFor(agentModel: string, tier?: string | null): Promise<string | null> {
  if (!tier) return agentModel
  const { agents } = await listFleetAgents()
  const a = agents.find((x) => x.id === agentModel)
  if (!a || !a.tiers.includes(tier)) return null
  return `${agentModel}-${tier}`
}
