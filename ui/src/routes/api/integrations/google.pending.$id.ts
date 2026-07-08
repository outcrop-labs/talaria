import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { decideAction } from '@/server/google/pending-actions'

const Body = z.object({ decision: z.enum(['approve', 'reject']) })

// POST /api/integrations/google/pending/$id { decision } → approve (executes as
// the owner) or reject an agent-drafted action.
export const Route = createFileRoute('/api/integrations/google/pending/$id')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })

        const outcome = await decideAction(params.id, user.id, parsed.data.decision, Date.now())
        if (!outcome) return json({ error: 'not found' }, { status: 404 })
        if (outcome.status === 'forbidden') return json({ error: 'forbidden' }, { status: 403 })
        if (outcome.status === 'not_connected') return json({ error: 'not_connected', message: outcome.message }, { status: 409 })
        if (outcome.status === 'failed') return json({ error: 'failed', message: outcome.message }, { status: 502 })
        return json({ status: outcome.status })
      },
    },
  },
})
