import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { getEmailConfig, setEmailConfig, sendEmail, emailShell } from '@/server/email'
import { logAudit } from '@/server/audit'

// Transactional email config. GET → config with secrets MASKED (set-flags
// only). POST → patch config; { test: true } → send a test to the caller.
export const Route = createFileRoute('/api/admin/email')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const cfg = await getEmailConfig()
        return json({
          config: {
            provider: cfg.provider,
            from: cfg.from,
            smtp: { host: cfg.smtp.host, port: cfg.smtp.port, secure: cfg.smtp.secure, user: cfg.smtp.user, passSet: !!cfg.smtp.passEnc },
            resend: { apiKeySet: !!cfg.resend.apiKeyEnc },
          },
        })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const actor = user.email ?? user.name ?? 'admin'
        const parsed = z
          .union([
            z.object({ test: z.literal(true) }),
            z.object({
              provider: z.enum(['smtp', 'resend']).nullable().optional(),
              from: z.string().max(200).optional(),
              smtp: z
                .object({
                  host: z.string().max(200).optional(),
                  port: z.number().int().min(1).max(65535).optional(),
                  secure: z.boolean().optional(),
                  user: z.string().max(200).optional(),
                  pass: z.string().max(500).nullable().optional(),
                })
                .optional(),
              resend: z.object({ apiKey: z.string().max(200).nullable().optional() }).optional(),
            }),
          ])
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if ('test' in parsed.data) {
          if (!user.email) return json({ error: 'your account has no email to test against' }, { status: 400 })
          const r = await sendEmail({
            to: user.email,
            subject: 'Talaria test email',
            html: emailShell('It works', '<p>Your transactional email configuration delivers. This is a test message from Talaria.</p>'),
            text: 'Your transactional email configuration delivers.',
          })
          return r.ok ? json({ ok: true }) : json({ error: r.error }, { status: 502 })
        }
        await setEmailConfig(parsed.data)
        void logAudit({ actor, action: 'email.config', targetType: 'email', targetId: 'config', after: { provider: parsed.data.provider } })
        return json({ ok: true })
      },
    },
  },
})
