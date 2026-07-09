import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { createChannel, listChannels, listChannelsForAgent } from '@/server/channels'
import { maybeSweepIdleChats } from '@/server/comms-decay'
import { ensureMcpService } from '@/server/mcp-service'

// GET /api/channels → the user's channels/relays/DMs. POST { name, topic?,
// kind? } → create a channel (default) or a Relay (kind 'group').
export const Route = createFileRoute('/api/channels')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        // Agents see the channels they've been added to.
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          return json({ channels: await listChannelsForAgent(name) })
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        maybeSweepIdleChats() // distill-then-archive idle agent DMs (throttled, detached)
        ensureMcpService() // keep the fleet's toolkit MCP endpoint alive (probe-guarded)
        return json({ channels: await listChannels(user.id) })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z
          .object({
            name: z.string().min(1).max(80),
            topic: z.string().max(300).nullish(),
            kind: z.enum(['channel', 'group']).optional(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        return json({
          channel: await createChannel(user.id, parsed.data.name, parsed.data.topic ?? null, parsed.data.kind ?? 'channel'),
        })
      },
    },
  },
})
