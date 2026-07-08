import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { listPending } from '@/server/google/pending-actions'

// GET /api/integrations/google/pending → the caller's agent-drafted actions
// awaiting their approval (send email / create event).
export const Route = createFileRoute('/api/integrations/google/pending')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ pending: await listPending(user.id) })
      },
    },
  },
})
