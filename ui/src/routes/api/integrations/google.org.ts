import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { disconnectOrg, getOrgConnectionStatus, setOrgTargets } from '@/server/google/org-connection'
import { googleIntegrationEnabled } from '@/server/google/oauth'

const Targets = z.object({
  driveFolderId: z.string().max(200).nullish(),
  calendarId: z.string().max(300).nullish(),
  sendAs: z.string().max(300).nullish(),
})

// The shared org Google connection (admin-managed). General fleet agents act as
// this identity for Drive/Docs/Calendar/Gmail.
// GET → status + targets · PUT → save build targets · DELETE → disconnect
export const Route = createFileRoute('/api/integrations/google/org')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ available: googleIntegrationEnabled(), ...(await getOrgConnectionStatus()) })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Targets.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await setOrgTargets(parsed.data)
        return json({ ok: true, ...(await getOrgConnectionStatus()) })
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
