import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { addChannelAgent, channelRole, removeChannelAgent } from '@/server/channels'
import { canUseAgentModel } from '@/server/users'

const Body = z.object({ model: z.string().min(1).max(200) })

// POST { model } → add a fleet agent to the channel (adder needs access to that
// agent). DELETE { model } → remove it. Any member.
export const Route = createFileRoute('/api/channels/$id/agents')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        // Personal assistants may join shared channels — but only their OWNER
        // can bring them (canUseAgentModel enforces that), and their group
        // replies carry the privacy gate (channel-replies): the owner's
        // private context never surfaces outside a DM with the owner.
        if (!(await canUseAgentModel(user.id, user.role, body.model))) {
          return json({ error: 'forbidden: no access to this agent' }, { status: 403 })
        }
        await addChannelAgent(params.id, body.model)
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        await removeChannelAgent(params.id, body.model)
        return json({ ok: true })
      },
    },
  },
})
