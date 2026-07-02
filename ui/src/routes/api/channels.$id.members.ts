import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { addChannelMember, channelRole, removeChannelMember } from '@/server/channels'

// POST { email } → add a member (any member can invite).
// DELETE { userId } → remove a member (owner, or yourself to leave).
export const Route = createFileRoute('/api/channels/$id/members')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ email: z.string().email() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const res = await addChannelMember(params.id, parsed.data.email)
        return res.ok ? json({ ok: true }) : json({ error: res.error }, { status: 400 })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const role = await channelRole(user.id, params.id)
        if (!role) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ userId: z.string().uuid() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if (role !== 'owner' && parsed.data.userId !== user.id) return json({ error: 'forbidden' }, { status: 403 })
        await removeChannelMember(params.id, parsed.data.userId)
        return json({ ok: true })
      },
    },
  },
})
