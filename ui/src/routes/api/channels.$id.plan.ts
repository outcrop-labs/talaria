import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
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
export const Route = createFileRoute('/api/channels/$id/plan')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })

        const agents = await listChannelAgents(params.id)
        if (!agents.includes(parsed.data.agentModel)) {
          return json({ error: 'that agent is not in this channel' }, { status: 400 })
        }
        if (!(await canUseAgentModel(user.id, user.role, parsed.data.agentModel))) {
          return json({ error: 'you do not have access to that agent' }, { status: 403 })
        }
        const routed =
          (parsed.data.tier ? await routedModelFor(parsed.data.agentModel, parsed.data.tier).catch(() => null) : null) ??
          parsed.data.agentModel

        try {
          const { proposals, raw } = await planFromChannel(params.id, parsed.data.agentModel, routed, {
            boardId: parsed.data.boardId,
            templateId: parsed.data.templateId,
          })
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
