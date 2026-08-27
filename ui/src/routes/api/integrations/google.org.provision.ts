import { defineApi } from '@/server/api-route'
import { parseBody, requireAdmin } from '@/server/api-guard'
import { json } from '@/server/http'
import { z } from 'zod'
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
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
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
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const parsed = await parseBody(request, Body)
    if (parsed instanceof Response) return parsed
    if (!parsed.calendar && !parsed.drive) return json({ error: 'bad request' }, { status: 400 })
    return json(await provisionWorkspace(parsed))
  },
})
