import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { auditRetentionDays, logAudit, setSetting } from '@/server/audit'
import { orgProfile, setOrgProfile } from '@/server/org'

// App settings (admin). GET → current values. PUT → update. Grows as more
// app-wide settings land; audit retention is the first.
export const Route = createFileRoute('/api/admin/settings')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ auditRetentionDays: await auditRetentionDays(), org: await orgProfile() })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({
            auditRetentionDays: z.number().int().min(0).max(3650).optional(),
            org: z.object({ name: z.string().max(120).optional(), about: z.string().max(2000).optional() }).optional(),
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
        if (parsed.data.org) {
          await setOrgProfile(parsed.data.org)
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
