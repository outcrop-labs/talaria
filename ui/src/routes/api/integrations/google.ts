import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { disconnect, getConnectionStatus } from '@/server/google/connections'
import { googleIntegrationEnabled } from '@/server/google/oauth'

// GET  → this user's Google connection status (never exposes tokens)
// DELETE → disconnect (revoke + forget)
export const Route = createFileRoute('/api/integrations/google')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const status = await getConnectionStatus(user.id)
        return json({ available: googleIntegrationEnabled(), ...status })
      },
      DELETE: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        await disconnect(user.id)
        return json({ ok: true })
      },
    },
  },
})
