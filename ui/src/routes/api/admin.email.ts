import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { getEmailConfig, setEmailConfig, sendEmail, emailShell } from '@/server/email'
import { logAudit } from '@/server/audit'

// Transactional email config. GET → config with secrets MASKED (set-flags
// only). POST → patch config; { test: true } → send a test to the caller.
export const Route = createFileRoute('/api/admin/email')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireAdmin(request)
        if (gate instanceof Response) return gate
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
      // PUT → config patch (the write); POST → send a test email (the action).
      PUT: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(
          request,
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
        )
        if (body instanceof Response) return body
        await setEmailConfig(body)
        void logAudit({ actor: actorOf(user), action: 'email.config', targetType: 'email', targetId: 'config', after: { provider: body.provider } })
        return json({ ok: true })
      },
      POST: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ test: z.literal(true) }))
        if (body instanceof Response) return body
        if (!user.email) return json({ error: 'your account has no email to test against' }, { status: 400 })
        const r = await sendEmail({
          to: user.email,
          subject: 'Talaria test email',
          html: emailShell('It works', '<p>Your transactional email configuration delivers. This is a test message from Talaria.</p>'),
          text: 'Your transactional email configuration delivers.',
        })
        return r.ok ? json({ ok: true }) : json({ error: r.error }, { status: 502 })
      },
    },
  },
})
