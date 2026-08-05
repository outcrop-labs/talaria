import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSessionUser } from '@/server/auth/session'
import { listPending } from '@/server/google/pending-actions'

// GET /api/integrations/google/pending → the caller's agent-drafted actions
// awaiting their approval (send email / create event).
export const Route = defineApi('/api/integrations/google/pending', {
  GET: async ({ request }) => {
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    return json({ pending: await listPending(user.id, user.role === 'admin') })
  },
})
