import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { computeAlerts } from '@/server/alerts'

// Derived system alerts (no persistence) for the requesting user.
export const Route = createFileRoute('/api/alerts')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ alerts: await computeAlerts(user.id) })
      },
    },
  },
})
