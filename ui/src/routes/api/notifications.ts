import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { listNotifications, markNotificationsRead, unreadCount } from '@/server/notifications'

// GET /api/notifications → the user's inbox + unread count.
// PUT { ids? } → mark those read (or all when ids is omitted).
export const Route = createFileRoute('/api/notifications')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json({
          notifications: await listNotifications(user.id),
          unread: await unreadCount(user.id),
        })
      },
      PUT: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ ids: z.array(z.string().uuid()).max(200).optional() }))
        if (body instanceof Response) return body
        await markNotificationsRead(user.id, body.ids)
        return json({ ok: true })
      },
    },
  },
})
