import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { reconcileFleet } from '@/server/fleet-reconcile'

// POST → render + start every enabled managed agent that isn't running. One
// button to bring the fleet to desired state (drift, cold start). Admin.
export const Route = createFileRoute('/api/fleet/reconcile')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        try {
          return json(await reconcileFleet())
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 500 })
        }
      },
    },
  },
})
