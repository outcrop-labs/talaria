import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { actorOf, requireAdmin } from '@/server/api-guard'
import { renderFleet } from '@/server/fleet-render'
import { logAudit } from '@/server/audit'

// POST → render every managed agent's config + the fleet compose + the gateway
// manifest (the bridge hot-reloads the manifest). Admin.
export const Route = defineApi('/api/fleet/render', {
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    try {
      const result = await renderFleet()
      void logAudit({ actor: actorOf(user), action: 'fleet.render', targetType: 'fleet', targetId: 'fleet' })
      return json({ result })
    } catch (e) {
      console.error('[fleet.render]', e)
      return json({ error: 'render failed — see server logs' }, { status: 500 })
    }
  },
})
