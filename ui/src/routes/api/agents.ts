import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { listFleetAgents } from '@/server/fleet-agents'
import { requireUser } from '@/server/api-guard'
import { usableAgentGate } from '@/server/users'

// GET /api/agents → the fleet the current user may use (definition-backed
// agents with their model tiers, filtered by per-agent access). Auth-gated.
export const Route = defineApi('/api/agents', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user

    const agents = await listFleetAgents()
    // Owner-aware: a personal assistant is only visible to its owner.
    const usable = await usableAgentGate(user.id, user.role)
    const visible = agents.filter((a) => usable(a.id))
    return json({ agents: visible })
  },
})
