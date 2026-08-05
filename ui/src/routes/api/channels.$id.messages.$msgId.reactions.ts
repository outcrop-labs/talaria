import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentCaller } from '@/server/agent-auth'
import { agentMayAccessChannel, channelRole, getChannelMessage, toggleReaction } from '@/server/channels'

// POST { emoji } → toggle your reaction on a message. Agents react too, under
// their own identity — one of our twists on the Slack shape.
export const Route = defineApi('/api/channels/$id/messages/$msgId/reactions', {
  POST: async ({ request, params }) => {
    const parsed = z
      .object({ emoji: z.string().min(1).max(16) })
      .safeParse(await request.json().catch(() => null))
    if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
    if (!(await getChannelMessage(params.id, params.msgId))) return json({ error: 'not found' }, { status: 404 })

    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    if (agent) {
      const name = agent.model
      // The CALLER, not `name`: elevation buys org-wide channel reach.
      if (!(await agentMayAccessChannel(params.id, agent))) return json({ error: 'forbidden' }, { status: 403 })
      await toggleReaction(params.id, params.msgId, parsed.data.emoji, name, 'agent')
      return json({ ok: true })
    }
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    await toggleReaction(params.id, params.msgId, parsed.data.emoji, user.email ?? user.name ?? 'user', 'user')
    return json({ ok: true })
  },
})
