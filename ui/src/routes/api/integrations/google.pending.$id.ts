import { defineApi } from '@/server/api-route'
import { parseBody, requireUser } from '@/server/api-guard'
import { json } from '@/server/http'
import { z } from 'zod'
import { decideAction } from '@/server/google/pending-actions'

const Body = z.object({ decision: z.enum(['approve', 'reject']) })

// POST /api/integrations/google/pending/$id { decision } → approve (executes as
// the owner) or reject an agent-drafted action.
export const Route = defineApi('/api/integrations/google/pending/$id', {
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const parsed = await parseBody(request, Body)
    if (parsed instanceof Response) return parsed

    const outcome = await decideAction(params.id, { id: user.id, isAdmin: user.role === 'admin' }, parsed.decision, Date.now())
    if (!outcome) return json({ error: 'not found' }, { status: 404 })
    if (outcome.status === 'forbidden') return json({ error: 'forbidden' }, { status: 403 })
    if (outcome.status === 'not_connected') return json({ error: 'not_connected', message: outcome.message }, { status: 409 })
    if (outcome.status === 'failed') return json({ error: 'failed', message: outcome.message }, { status: 502 })
    return json({ status: outcome.status })
  },
})
