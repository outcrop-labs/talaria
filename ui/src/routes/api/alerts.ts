import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { computeAlerts } from '@/server/alerts'

// Derived system alerts (no persistence) for the requesting user.
export const Route = defineApi('/api/alerts', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({ alerts: await computeAlerts(user.id) })
  },
})
