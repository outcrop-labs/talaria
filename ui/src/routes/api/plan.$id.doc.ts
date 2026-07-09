import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { ownedConversation } from '@/server/conversations'
import { syncPlanDoc } from '@/server/plan-doc'
import { routedModelFor } from '@/server/fleet-agents'
import { canUseAgentModel } from '@/server/users'

const Body = z.object({ tier: z.string().max(60).nullish() })

// POST → the plan's agent rewrites the living plan document from the
// conversation so far. Owner only; the document is the plan's linked artifact.
export const Route = createFileRoute('/api/plan/$id/doc')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const owned = await ownedConversation(user.id, params.id)
        if (!owned || owned.kind !== 'plan') return json({ error: 'plan not found' }, { status: 404 })
        if (!(await canUseAgentModel(user.id, user.role, owned.agentModel))) {
          return json({ error: 'you do not have access to that agent' }, { status: 403 })
        }
        const parsed = Body.safeParse(await request.json().catch(() => ({})))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const routed =
          (parsed.data.tier ? await routedModelFor(owned.agentModel, parsed.data.tier).catch(() => null) : null) ??
          owned.agentModel
        try {
          const artifact = await syncPlanDoc(
            params.id,
            { id: user.id, label: user.name ?? user.email ?? 'someone' },
            owned.title,
            owned.agentModel,
            routed,
          )
          return json({ artifactId: artifact.id })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }
      },
    },
  },
})
