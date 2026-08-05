import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireView } from '@/server/api-guard'
import { getFleetOverview } from '@/server/fleet'

// GET /api/fleet → owned fleet ops data (agents + Talaria-native usage).
// Ops-wide detail: admins + people granted the Observability view.
export const Route = defineApi('/api/fleet', {
  GET: async ({ request }) => {
    const gate = await requireView(request, '/observability')
    if (gate instanceof Response) return gate
    return json(await getFleetOverview())
  },
})
