// Fleet ops data — OWNED by Talaria (not proxied from mission-control). The agent
// list comes from the gateway plane (/v1/models); usage comes from Talaria's own
// Postgres (conversations + messages). As we rip more of the "brain" in (agent
// registry/heartbeat, task queue, token ledger — agents reporting to Talaria),
// this grows from usage stats into full fleet telemetry.

import { db } from './db/pg'
import { listFleetAgents } from './fleet-agents'
import { listAgentDefs } from './agent-defs'
import { containerStatus } from './fleet-docker'
import { registryByName, seedFleetNames, type AgentStatus } from './agents-registry'

export interface FleetAgentStat {
  id: string
  label: string
  role: string
  status: AgentStatus
  lastSeen: string | null
  lastActivity: string | null
  conversations: number
  messages: number
  lastUsed: string | null
}

export interface FleetOverview {
  agents: FleetAgentStat[]
  source: 'gateway' | 'mock'
  totals: { agents: number; online: number; conversations: number; messages: number; activeToday: number }
}

export async function getFleetOverview(): Promise<FleetOverview> {
  const { agents, source } = await listFleetAgents()
  // Seed the registry from the fleet so every agent shows (offline until it
  // heartbeats to Talaria), then read owned status.
  await seedFleetNames(agents.map((a) => a.id))
  const registry = await registryByName()
  const sql = await db()

  // Heartbeats are the ideal liveness signal, but most agents don't heartbeat
  // to Talaria yet. Container reality (the same source as the roster status
  // dots) fills the gap, so the "online" count can never disagree with the
  // green dots on the agents page: running container ⇒ online.
  const defs = (await listAgentDefs().catch(() => [])).filter((d) => d.enabled)
  const containers = defs.length
    ? await containerStatus(defs.map((d) => d.department)).catch(() => [])
    : []
  const byDept = new Map(containers.map((c) => [c.department, c]))
  const containerUp = new Map(
    defs.map((d) => {
      const c = byDept.get(d.department)
      const state = c?.managed
      return [d.model, state?.state === 'running'] as const
    }),
  )

  // Fleet-wide usage (all users) per agent — the ops/maintainer view.
  const rows = await sql`
    select c.agent_model as model,
           count(distinct c.id)::int as conversations,
           count(m.id)::int as messages,
           max(c.updated_at) as last_used
    from conversations c
    left join messages m on m.conversation_id = c.id
    group by c.agent_model
  `
  const byModel = new Map(rows.map((r) => [r.model as string, r]))

  const stats: FleetAgentStat[] = agents.map((a) => {
    const r = byModel.get(a.id) as { conversations: number; messages: number; last_used: string | null } | undefined
    const reg = registry.get(a.id)
    const regStatus = reg?.status ?? 'offline'
    return {
      id: a.id,
      label: a.label,
      role: a.role,
      status: regStatus === 'offline' && containerUp.get(a.id) ? 'idle' : regStatus,
      lastSeen: reg?.lastSeen ?? null,
      lastActivity: reg?.lastActivity ?? null,
      conversations: r?.conversations ?? 0,
      messages: r?.messages ?? 0,
      lastUsed: r?.last_used ?? null,
    }
  })

  const dayAgo = Date.now() - 24 * 60 * 60 * 1000
  const totals = {
    agents: stats.length,
    online: stats.filter((s) => s.status !== 'offline').length,
    conversations: stats.reduce((n, s) => n + s.conversations, 0),
    messages: stats.reduce((n, s) => n + s.messages, 0),
    activeToday: stats.filter((s) => s.lastUsed && new Date(s.lastUsed).getTime() > dayAgo).length,
  }

  return { agents: stats, source, totals }
}
