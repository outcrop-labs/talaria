import { defineApi } from '@/server/api-route'
import { requireUser } from '@/server/api-guard'
import { json } from '@/server/http'
import { listPending } from '@/server/google/pending-actions'

// GET /api/integrations/google/pending → the caller's agent-drafted actions
// awaiting their approval (send email / create event).
export const Route = defineApi('/api/integrations/google/pending', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({ pending: await listPending(user.id, user.role === 'admin') })
  },
})
