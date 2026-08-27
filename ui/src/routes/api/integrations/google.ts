import { defineApi } from '@/server/api-route'
import { requireUser } from '@/server/api-guard'
import { json } from '@/server/http'
import { disconnect, getConnectionStatus } from '@/server/google/connections'
import { googleIntegrationEnabled } from '@/server/google/oauth'

// GET  → this user's Google connection status (never exposes tokens)
// DELETE → disconnect (revoke + forget)
export const Route = defineApi('/api/integrations/google', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const status = await getConnectionStatus(user.id)
    return json({ available: await googleIntegrationEnabled(), ...status })
  },
  DELETE: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    await disconnect(user.id)
    return json({ ok: true })
  },
})
