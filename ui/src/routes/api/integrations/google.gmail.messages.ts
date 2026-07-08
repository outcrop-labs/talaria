import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { listRecentMessages } from '@/server/google/gmail'
import { googleFail } from '@/server/google/errors'

// GET /api/integrations/google/gmail/messages?q= → recent mail (metadata only).
export const Route = createFileRoute('/api/integrations/google/gmail/messages')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const q = new URL(request.url).searchParams.get('q') || 'in:inbox'
        try {
          return json({ messages: await listRecentMessages(user.id, Date.now(), 8, q) })
        } catch (err) {
          return googleFail(err as Error, 'Gmail')
        }
      },
    },
  },
})
