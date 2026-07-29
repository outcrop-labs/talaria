import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireView } from '@/server/api-guard'
import { costOverview } from '@/server/usage'

// GET /api/cost → the token ledger overview (totals, per-agent, per-day).
// Org-wide financials: admins + people granted the Observability view.
export const Route = createFileRoute('/api/cost')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireView(request, '/observability')
        if (gate instanceof Response) return gate
        return json(await costOverview())
      },
    },
  },
})
