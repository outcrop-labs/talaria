import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireView } from '@/server/api-guard'
import { getFleetOverview } from '@/server/fleet'

// GET /api/fleet → owned fleet ops data (agents + Talaria-native usage).
// Ops-wide detail: admins + people granted the Observability view.
export const Route = createFileRoute('/api/fleet')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireView(request, '/observability')
        if (gate instanceof Response) return gate
        return json(await getFleetOverview())
      },
    },
  },
})
