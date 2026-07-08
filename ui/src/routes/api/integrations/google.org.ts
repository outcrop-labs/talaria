import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { disconnectOrg, getOrgConnectionStatus } from '@/server/google/org-connection'
import { googleIntegrationEnabled } from '@/server/google/oauth'

// The shared org Google connection (admin-managed). General fleet agents act as
// this identity for Drive/Docs.
// GET → status · DELETE → disconnect
export const Route = createFileRoute('/api/integrations/google/org')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ available: googleIntegrationEnabled(), ...(await getOrgConnectionStatus()) })
      },
      DELETE: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        await disconnectOrg()
        return json({ ok: true })
      },
    },
  },
})
