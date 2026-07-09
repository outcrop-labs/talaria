import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { ownedConversationModel } from '@/server/conversations'
import { planFromConversation } from '@/server/channel-plan'
import { routedModelFor } from '@/server/fleet-agents'
import { canUseAgentModel } from '@/server/users'

const Body = z.object({ tier: z.string().max(60).nullish() })

// POST → draft ticket proposals from a plan conversation. Owner only; nothing is
// created here — the human reviews and creates via the boards API (PlanModal).
export const Route = createFileRoute('/api/plan/$id/draft')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const agentModel = await ownedConversationModel(user.id, params.id)
        if (!agentModel) return json({ error: 'plan not found' }, { status: 404 })
        if (!(await canUseAgentModel(user.id, user.role, agentModel))) {
          return json({ error: 'you do not have access to that agent' }, { status: 403 })
        }
        const parsed = Body.safeParse(await request.json().catch(() => ({})))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const routed =
          (parsed.data.tier ? await routedModelFor(agentModel, parsed.data.tier).catch(() => null) : null) ?? agentModel
        try {
          const { proposals, raw } = await planFromConversation(params.id, agentModel, routed)
          if (proposals.length === 0) {
            return json({ proposals: [], note: raw ? 'the agent did not return parseable tickets' : 'nothing to plan yet' })
          }
          return json({ proposals })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }
      },
    },
  },
})
