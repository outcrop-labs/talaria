import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { addChannelAgent, channelRole, removeChannelAgent } from '@/server/channels'
import { canUseAgentModel, personalAssistantOwners } from '@/server/users'

const Body = z.object({ model: z.string().min(1).max(200) })

// POST { model } → add a fleet agent to the channel (adder needs access to that
// agent). DELETE { model } → remove it. Any member.
export const Route = createFileRoute('/api/channels/$id/agents')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // A personal assistant acts AS its owner (their Google, memory, private
        // soul); it must never live in a shared channel where other members could
        // prompt it. Block it regardless of who's adding.
        if ((await personalAssistantOwners()).has(parsed.data.model)) {
          return json({ error: 'forbidden: personal assistants can’t be added to shared channels' }, { status: 403 })
        }
        if (!(await canUseAgentModel(user.id, user.role, parsed.data.model))) {
          return json({ error: 'forbidden: no access to this agent' }, { status: 403 })
        }
        await addChannelAgent(params.id, parsed.data.model)
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await removeChannelAgent(params.id, parsed.data.model)
        return json({ ok: true })
      },
    },
  },
})
