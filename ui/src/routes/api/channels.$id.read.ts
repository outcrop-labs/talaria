import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { channelRole, markChannelRead } from '@/server/channels'

// POST { seq } → advance the caller's read cursor (drives unread badges).
export const Route = createFileRoute('/api/channels/$id/read')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z.object({ seq: z.number().int().min(0) }).safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await markChannelRead(params.id, user.id, parsed.data.seq)
        return json({ ok: true })
      },
    },
  },
})
