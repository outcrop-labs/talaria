import { defineApi } from '@/server/api-route'
import { parseBody, requireAdmin } from '@/server/api-guard'
import { json } from '@/server/http'
import { z } from 'zod'
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
export const Route = defineApi('/api/integrations/google/org', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    return json({ available: await googleIntegrationEnabled(), ...(await getOrgConnectionStatus()) })
  },
  PUT: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const parsed = await parseBody(request, Targets)
    if (parsed instanceof Response) return parsed
    await setOrgTargets(parsed)
    return json({ ok: true, ...(await getOrgConnectionStatus()) })
  },
  DELETE: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    await disconnectOrg()
    return json({ ok: true })
  },
})
