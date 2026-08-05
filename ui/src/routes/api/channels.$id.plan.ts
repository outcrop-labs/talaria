import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { channelRole, listChannelAgents } from '@/server/channels'
import { planFromChannel } from '@/server/channel-plan'
import { routedModelFor } from '@/server/fleet-agents'
import { canUseAgentModel } from '@/server/users'

const Body = z.object({
  agentModel: z.string().min(1).max(200),
  tier: z.string().max(60).nullish(),
  /** Where the tickets will land — lets the board's default template apply. */
  boardId: z.string().uuid().nullish(),
  /** Explicit template pick (overrides agent/board resolution). */
  templateId: z.string().uuid().nullish(),
})

// POST → ask a channel agent to draft ticket proposals from the transcript.
// Members only; the agent must be in the channel and usable by the caller.
// Nothing is created here — the human reviews and creates via the boards API.
export const Route = defineApi('/api/channels/$id/plan', {
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body

    const agents = await listChannelAgents(params.id)
    if (!agents.includes(body.agentModel)) {
      return json({ error: 'that agent is not in this channel' }, { status: 400 })
    }
    if (!(await canUseAgentModel(user.id, user.role, body.agentModel))) {
      return json({ error: 'you do not have access to that agent' }, { status: 403 })
    }
    const routed =
      (body.tier ? await routedModelFor(body.agentModel, body.tier).catch(() => null) : null) ??
      body.agentModel

    try {
      const { proposals, raw } = await planFromChannel(params.id, body.agentModel, routed, {
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
