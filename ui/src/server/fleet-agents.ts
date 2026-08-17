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

/** The agents that EXIST, which is a question only `agent_defs` can answer.
 *
 *  `fleet/fleet.json` is a render OUTPUT — the transport table of where each
 *  agent's gateway lives — not a roster. Treating it as one meant a manifest
 *  left on disk resurrected agents the database no longer had: delete every
 *  agent, or restore a checkout onto a fresh database, and the stale file put
 *  them all back, complete with their old gateway keys.
 *
 *  The `defs.length === 0` early return made that worse rather than better. It
 *  was meant as a bootstrap convenience — "the DB isn't populated yet, don't
 *  filter" — but an empty definition table does not mean "unknown", it means
 *  NONE, and returning the whole manifest for it is the one case where the
 *  answer must be empty. So the filter is unconditional now: the manifest can
 *  only ever narrow what the database already claims. */
export async function listFleetAgents(): Promise<FleetAgentEntry[]> {
  const agents = await listAgents()
  const sql = await db()
  const defs = (await sql`
    select d.model, v.config
    from agent_defs d
    left join agent_versions v on v.agent_id = d.id and v.version = d.current_version
    where d.enabled
  `) as unknown as Array<{ model: string; config: AgentConfig | null }>
  const byModel = new Map(defs.map((d) => [d.model, d.config?.aliases?.map((a) => a.name) ?? []]))
  return agents.filter((a) => byModel.has(a.id)).map((a) => ({ ...a, tiers: byModel.get(a.id)! }))
}

/** Validate a tier for an agent; returns the routed gateway model id. */
export async function routedModelFor(agentModel: string, tier?: string | null): Promise<string | null> {
  if (!tier) return agentModel
  const agents = await listFleetAgents()
  const a = agents.find((x) => x.id === agentModel)
  if (!a || !a.tiers.includes(tier)) return null
  return `${agentModel}-${tier}`
}
