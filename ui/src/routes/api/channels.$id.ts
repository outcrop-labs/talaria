import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import {
  archiveChannel,
  channelRole,
  deleteChannel,
  listChannelAgents,
  listChannelMembers,
  updateChannel,
} from '@/server/channels'
import { purgeActivityByField } from '@/server/retrieval/sources'

// GET → channel detail (members + agents). PUT → rename / set topic (owner).
// DELETE → archive (?hard=1 deletes; owner only).
export const Route = createFileRoute('/api/channels/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const role = await channelRole(user.id, params.id)
        if (!role) return json({ error: 'forbidden' }, { status: 403 })
        return json({
          role,
          members: await listChannelMembers(params.id),
          agents: await listChannelAgents(params.id),
        })
      },
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if ((await channelRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ name: z.string().min(1).max(80).optional(), topic: z.string().max(300).nullish() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await updateChannel(params.id, { name: parsed.data.name, topic: parsed.data.topic })
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if ((await channelRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
        const hard = new URL(request.url).searchParams.get('hard') === '1'
        await (hard ? deleteChannel(params.id) : archiveChannel(params.id))
        // A hard delete removes the channel's messages — purge their activity
        // points too so nothing is orphaned in the index.
        if (hard) void purgeActivityByField('channelId', params.id).catch(() => {})
        return json({ ok: true })
      },
    },
  },
})
