import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentFromAddress } from '@/server/google/aliasing'
import { listAgentDefs } from '@/server/agent-defs'
import { getOrgConnectionStatus, getOrgEmail } from '@/server/google/org-connection'
import { provisionWorkspace, provisioningReadiness } from '@/server/google/provisioning'

const Body = z.object({ calendar: z.boolean().optional(), drive: z.boolean().optional() })

// The org workspace provisioning surface (admin). GET → what the panel draws:
// scope readiness, the provisioned container ids, and every agent's effective
// send address. POST → run the requested provisions, per-item outcomes back.
export const Route = defineApi('/api/integrations/google/org/provision', {
  GET: async ({ request }) => {
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
    const [readiness, status, orgEmail, defs] = await Promise.all([
      provisioningReadiness(),
      getOrgConnectionStatus(),
      getOrgEmail(),
      listAgentDefs(),
    ])
    return json({
      readiness,
      orgEmail,
      calendarId: status.targets.calendarId,
      sharedDriveId: status.targets.sharedDriveId,
      agents: defs
        .filter((d) => !d.ownerUserId) // org agents send as the org; a personal assistant sends as its owner
        .map((d) => ({
          model: d.model,
          displayName: d.displayName,
          slug: d.slug,
          alias: d.emailAlias ?? null,
          effective: agentFromAddress({ slug: d.slug, emailAlias: d.emailAlias }, orgEmail),
        })),
    })
  },
  POST: async ({ request }) => {
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
    const parsed = Body.safeParse(await request.json().catch(() => null))
    if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
    if (!parsed.data.calendar && !parsed.data.drive) return json({ error: 'bad request' }, { status: 400 })
    return json(await provisionWorkspace(parsed.data))
  },
})
