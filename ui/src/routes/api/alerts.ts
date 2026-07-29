import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { computeAlerts } from '@/server/alerts'

// Derived system alerts (no persistence) for the requesting user.
export const Route = createFileRoute('/api/alerts')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json({ alerts: await computeAlerts(user.id) })
      },
    },
  },
})
