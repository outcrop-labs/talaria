import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import {
  archiveChannel,
  channelRole,
  deleteChannel,
  listChannelAgents,
  listChannelMembers,
  updateChannel,
} from '@/server/channels'
import { purgeActivityByField } from '@/server/retrieval/sources'

const Put = z.object({ name: z.string().min(1).max(80).optional(), topic: z.string().max(300).nullish() })

// GET → channel detail (members + agents). PUT → rename / set topic (owner).
// DELETE → archive (?hard=1 deletes; owner only).
export const Route = defineApi('/api/channels/$id', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const role = await channelRole(user.id, params.id)
    if (!role) return json({ error: 'forbidden' }, { status: 403 })
    return json({
      role,
      members: await listChannelMembers(params.id),
      agents: await listChannelAgents(params.id),
    })
  },
  PUT: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if ((await channelRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Put)
    if (body instanceof Response) return body
    await updateChannel(params.id, { name: body.name, topic: body.topic })
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if ((await channelRole(user.id, params.id)) !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
    const hard = new URL(request.url).searchParams.get('hard') === '1'
    await (hard ? deleteChannel(params.id) : archiveChannel(params.id))
    // A hard delete removes the channel's messages — purge their activity
    // points too so nothing is orphaned in the index.
    if (hard) void purgeActivityByField('channelId', params.id).catch(() => {})
    return json({ ok: true })
  },
})
