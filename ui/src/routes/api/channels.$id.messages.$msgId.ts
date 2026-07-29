import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import {
  channelRole,
  deleteChannelMessage,
  editChannelMessage,
  getChannelMessage,
} from '@/server/channels'

// PATCH { content } → edit your own message (edited marker shows).
// DELETE → remove it: the author, or the channel owner tidying up. A thread
// root takes its replies with it.
export const Route = createFileRoute('/api/channels/$id/messages/$msgId')({
  server: {
    handlers: {
      PATCH: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const msg = await getChannelMessage(params.id, params.msgId)
        if (!msg) return json({ error: 'not found' }, { status: 404 })
        const author = user.email ?? user.name ?? 'user'
        if (msg.authorType !== 'user' || msg.author !== author) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ content: z.string().trim().min(1).max(20_000) })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await editChannelMessage(params.id, params.msgId, parsed.data.content)
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const role = await channelRole(user.id, params.id)
        if (!role) return json({ error: 'forbidden' }, { status: 403 })
        const msg = await getChannelMessage(params.id, params.msgId)
        if (!msg) return json({ error: 'not found' }, { status: 404 })
        const author = user.email ?? user.name ?? 'user'
        const own = msg.authorType === 'user' && msg.author === author
        if (!own && role !== 'owner') return json({ error: 'forbidden' }, { status: 403 })
        await deleteChannelMessage(params.id, params.msgId)
        return json({ ok: true })
      },
    },
  },
})
