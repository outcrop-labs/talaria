import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { actorOf, requireAdmin } from '@/server/api-guard'
import { reconcileFleet } from '@/server/fleet-reconcile'
import { logAudit } from '@/server/audit'

// POST → render + start every enabled managed agent that isn't running. One
// button to bring the fleet to desired state (drift, cold start). Admin.
export const Route = defineApi('/api/fleet/reconcile', {
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    try {
      const result = await reconcileFleet()
      void logAudit({ actor: actorOf(user), action: 'fleet.reconcile', targetType: 'fleet', targetId: 'fleet' })
      return json(result)
    } catch (e) {
      console.error('[fleet.reconcile]', e)
      return json({ error: 'reconcile failed — see server logs' }, { status: 500 })
    }
  },
})
