import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { addChannelMember, channelRole, removeChannelMember } from '@/server/channels'

const Post = z.object({ email: z.string().email() })
const Delete = z.object({ userId: z.string().uuid() })

// POST { email } → add a member (any member can invite).
// DELETE { userId } → remove a member (owner, or yourself to leave).
export const Route = defineApi('/api/channels/$id/members', {
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Post)
    if (body instanceof Response) return body
    const res = await addChannelMember(params.id, body.email)
    return res.ok ? json({ ok: true }) : json({ error: res.error }, { status: 400 })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const role = await channelRole(user.id, params.id)
    if (!role) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Delete)
    if (body instanceof Response) return body
    if (role !== 'owner' && body.userId !== user.id) return json({ error: 'forbidden' }, { status: 403 })
    await removeChannelMember(params.id, body.userId)
    return json({ ok: true })
  },
})
