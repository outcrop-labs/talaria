import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { listNotifications, markNotificationsRead, unreadCount } from '@/server/notifications'

// GET /api/notifications → the user's inbox + unread count.
// PUT { ids? } → mark those read (or all when ids is omitted).
export const Route = createFileRoute('/api/notifications')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({
          notifications: await listNotifications(user.id),
          unread: await unreadCount(user.id),
        })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z
          .object({ ids: z.array(z.string().uuid()).max(200).optional() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await markNotificationsRead(user.id, parsed.data.ids)
        return json({ ok: true })
      },
    },
  },
})
