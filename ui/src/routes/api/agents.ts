import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { listFleetAgents } from '@/server/fleet-agents'
import { requireUser } from '@/server/api-guard'
import { usableAgentGate } from '@/server/users'

// GET /api/agents → the fleet the current user may use (definition-backed
// agents with their model tiers, filtered by per-agent access). Auth-gated.
export const Route = createFileRoute('/api/agents')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user

        const { agents, source } = await listFleetAgents()
        // Owner-aware: a personal assistant is only visible to its owner.
        const usable = await usableAgentGate(user.id, user.role)
        const visible = agents.filter((a) => usable(a.id))
        return json({ agents: visible, source })
      },
    },
  },
})
