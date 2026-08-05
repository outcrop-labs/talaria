import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireView } from '@/server/api-guard'
import { costOverview } from '@/server/usage'

// GET /api/cost → the token ledger overview (totals, per-agent, per-day).
// Org-wide financials: admins + people granted the Observability view.
export const Route = defineApi('/api/cost', {
  GET: async ({ request }) => {
    const gate = await requireView(request, '/observability')
    if (gate instanceof Response) return gate
    return json(await costOverview())
  },
})
