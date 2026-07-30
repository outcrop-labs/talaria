import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm } from '@/server/api-guard'
import { getAgentDef, updateAgentMeta } from '@/server/agent-defs'
import { setAgentWorkbench } from '@/server/workbench'
import { setAgentTemplates } from '@/server/templates'
import { logAudit } from '@/server/audit'

const Body = z.object({
  role: z.string().max(80).nullish(),
  displayName: z.string().min(1).max(80).optional(),
  /** Template overrides: uuid binds, null clears, omitted leaves unchanged. */
  ticketTemplateId: z.string().uuid().nullable().optional(),
  planTemplateId: z.string().uuid().nullable().optional(),
  workbench: z.enum(['off', 'auto', 'on']).optional(),
  workbenchProfile: z.string().max(40).nullable().optional(),
})

// PATCH → editable agent identity metadata (role, display name). Not versioned
// — this is identity, not config. Admin only.
export const Route = createFileRoute('/api/fleet/defs/$id')({
  server: {
    handlers: {
      PATCH: async ({ request, params }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })
        await updateAgentMeta(params.id, { role: body.role, displayName: body.displayName })
        if (body.workbench !== undefined || body.workbenchProfile !== undefined) {
          await setAgentWorkbench(params.id, body.workbench ?? (def as unknown as { workbench?: 'off' | 'auto' | 'on' }).workbench ?? 'auto', body.workbenchProfile)
        }
        if (body.ticketTemplateId !== undefined || body.planTemplateId !== undefined) {
          await setAgentTemplates(def.model, {
            ticketTemplateId: body.ticketTemplateId,
            planTemplateId: body.planTemplateId,
          })
        }
        void logAudit({
          actor: actorOf(user),
          action: 'agent.meta',
          targetType: 'agent',
          targetId: def.id,
          targetLabel: def.displayName,
        })
        return json({ ok: true })
      },
    },
  },
})
