import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { auditRetentionDays, logAudit, setSetting } from '@/server/audit'

// App settings (admin). GET → current values. PUT → update. Grows as more
// app-wide settings land; audit retention is the first.
export const Route = createFileRoute('/api/admin/settings')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ auditRetentionDays: await auditRetentionDays() })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ auditRetentionDays: z.number().int().min(0).max(3650) })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await setSetting('audit_retention_days', parsed.data.auditRetentionDays)
        void logAudit({
          actor: user.email ?? user.name ?? 'admin',
          action: 'settings.audit_retention',
          targetType: 'settings',
          after: { auditRetentionDays: parsed.data.auditRetentionDays },
        })
        return json({ ok: true })
      },
    },
  },
})
