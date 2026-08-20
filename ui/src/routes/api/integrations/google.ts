import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSessionUser } from '@/server/auth/session'
import { disconnect, getConnectionStatus } from '@/server/google/connections'
import { googleIntegrationEnabled } from '@/server/google/oauth'

// GET  → this user's Google connection status (never exposes tokens)
// DELETE → disconnect (revoke + forget)
export const Route = defineApi('/api/integrations/google', {
  GET: async ({ request }) => {
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    const status = await getConnectionStatus(user.id)
    return json({ available: await googleIntegrationEnabled(), ...status })
  },
  DELETE: async ({ request }) => {
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    await disconnect(user.id)
    return json({ ok: true })
  },
})
