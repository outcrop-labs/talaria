import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import {
  channelRole,
  deleteChannelMessage,
  editChannelMessage,
  getChannelMessage,
} from '@/server/channels'

const Patch = z.object({ content: z.string().trim().min(1).max(20_000) })

// PATCH { content } → edit your own message (edited marker shows).
// DELETE → remove it: the author, or the channel owner tidying up. A thread
// root takes its replies with it.
export const Route = createFileRoute('/api/channels/$id/messages/$msgId')({
  server: {
    handlers: {
      PATCH: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const msg = await getChannelMessage(params.id, params.msgId)
        if (!msg) return json({ error: 'not found' }, { status: 404 })
        const author = user.email ?? user.name ?? 'user'
        if (msg.authorType !== 'user' || msg.author !== author) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Patch)
        if (body instanceof Response) return body
        await editChannelMessage(params.id, params.msgId, body.content)
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
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
