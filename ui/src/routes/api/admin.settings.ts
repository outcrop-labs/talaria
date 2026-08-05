import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { auditRetentionDays, logAudit, setSetting } from '@/server/audit'
import { orgProfile, setOrgProfile } from '@/server/org'
import { memberModelAllowlist, setMemberModelAllowlist } from '@/server/model-access'
import { rollRunningAgents } from '@/server/fleet-reconcile'

// App settings (admin). GET → current values. PUT → update. Grows as more
// app-wide settings land; audit retention is the first.
export const Route = defineApi('/api/admin/settings', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    return json({
      auditRetentionDays: await auditRetentionDays(),
      org: await orgProfile(),
      memberModels: await memberModelAllowlist(),
    })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({
        auditRetentionDays: z.number().int().min(0).max(3650).optional(),
        org: z.object({ name: z.string().max(120).optional(), about: z.string().max(2000).optional() }).optional(),
        /** Bare model ids members may pick; empty = all models. */
        memberModels: z.array(z.string().min(1).max(200)).max(200).optional(),
      }),
    )
    if (body instanceof Response) return body
    if (body.auditRetentionDays !== undefined) {
      await setSetting('audit_retention_days', body.auditRetentionDays)
      void logAudit({
        actor: actorOf(user),
        action: 'settings.audit_retention',
        targetType: 'settings',
        after: { auditRetentionDays: body.auditRetentionDays },
      })
    }
    if (body.memberModels !== undefined) {
      await setMemberModelAllowlist(body.memberModels)
      void logAudit({
        actor: actorOf(user),
        action: 'settings.member_models',
        targetType: 'settings',
        after: { memberModels: body.memberModels },
      })
    }
    if (body.org) {
      await setOrgProfile(body.org)
      // The org lives in every rendered soul — propagate by ROLLING running
      // agents (new container up + healthy before the old one retires), so
      // an identity edit never kills anyone's in-flight conversation.
      void rollRunningAgents().catch(() => {})
      void logAudit({
        actor: actorOf(user),
        action: 'settings.org',
        targetType: 'settings',
        after: body.org,
      })
    }
    return json({ ok: true })
  },
})
