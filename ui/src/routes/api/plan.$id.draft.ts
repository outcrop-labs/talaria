import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { accessibleConversation } from '@/server/conversations'
import { planFromConversation } from '@/server/channel-plan'
import { routedModelFor } from '@/server/fleet-agents'
import { canUseAgentModel } from '@/server/users'

const Body = z.object({
  tier: z.string().max(60).nullish(),
  /** Where the tickets will land — lets the board's default template apply. */
  boardId: z.string().uuid().nullish(),
  /** Explicit template pick (overrides agent/board resolution). */
  templateId: z.string().uuid().nullish(),
})

// POST → draft ticket proposals from a plan conversation. Owner only; nothing is
// created here — the human reviews and creates via the boards API (PlanModal).
export const Route = defineApi('/api/plan/$id/draft', {
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const agentModel = (await accessibleConversation(user.id, params.id))?.agentModel ?? null
    if (!agentModel) return json({ error: 'plan not found' }, { status: 404 })
    if (!(await canUseAgentModel(user.id, user.role, agentModel))) {
      return json({ error: 'you do not have access to that agent' }, { status: 403 })
    }
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const routed =
      (body.tier ? await routedModelFor(agentModel, body.tier).catch(() => null) : null) ?? agentModel
    try {
      const { proposals, raw } = await planFromConversation(params.id, agentModel, routed, {
        boardId: body.boardId,
        templateId: body.templateId,
      })
      if (proposals.length === 0) {
        return json({ proposals: [], note: raw ? 'the agent did not return parseable tickets' : 'nothing to plan yet' })
      }
      return json({ proposals })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 502 })
    }
  },
})
