import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { auditRetentionDays, logAudit, setSetting } from '@/server/audit'
import { orgProfile, setOrgProfile } from '@/server/org'
import { memberModelAllowlist, setMemberModelAllowlist } from '@/server/model-access'
import { rollRunningAgents } from '@/server/fleet-reconcile'

// App settings (admin). GET → current values. PUT → update. Grows as more
// app-wide settings land; audit retention is the first.
export const Route = createFileRoute('/api/admin/settings')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({
          auditRetentionDays: await auditRetentionDays(),
          org: await orgProfile(),
          memberModels: await memberModelAllowlist(),
        })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({
            auditRetentionDays: z.number().int().min(0).max(3650).optional(),
            org: z.object({ name: z.string().max(120).optional(), about: z.string().max(2000).optional() }).optional(),
            /** Bare model ids members may pick; empty = all models. */
            memberModels: z.array(z.string().min(1).max(200)).max(200).optional(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if (parsed.data.auditRetentionDays !== undefined) {
          await setSetting('audit_retention_days', parsed.data.auditRetentionDays)
          void logAudit({
            actor: user.email ?? user.name ?? 'admin',
            action: 'settings.audit_retention',
            targetType: 'settings',
            after: { auditRetentionDays: parsed.data.auditRetentionDays },
          })
        }
        if (parsed.data.memberModels !== undefined) {
          await setMemberModelAllowlist(parsed.data.memberModels)
          void logAudit({
            actor: user.email ?? user.name ?? 'admin',
            action: 'settings.member_models',
            targetType: 'settings',
            after: { memberModels: parsed.data.memberModels },
          })
        }
        if (parsed.data.org) {
          await setOrgProfile(parsed.data.org)
          // The org lives in every rendered soul — propagate by ROLLING running
          // agents (new container up + healthy before the old one retires), so
          // an identity edit never kills anyone's in-flight conversation.
          void rollRunningAgents().catch(() => {})
          void logAudit({
            actor: user.email ?? user.name ?? 'admin',
            action: 'settings.org',
            targetType: 'settings',
            after: parsed.data.org,
          })
        }
        return json({ ok: true })
      },
    },
  },
})
