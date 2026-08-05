import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { channelRole, markChannelRead } from '@/server/channels'

const Post = z.object({ seq: z.number().int().min(0) })

// POST { seq } → advance the caller's read cursor (drives unread badges).
export const Route = defineApi('/api/channels/$id/read', {
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Post)
    if (body instanceof Response) return body
    await markChannelRead(params.id, user.id, body.seq)
    return json({ ok: true })
  },
})
