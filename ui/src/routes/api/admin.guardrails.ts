import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { logAudit } from '@/server/audit'
import { getGuardConfig, guardRuleMeta, guardStats, listFindings, setGuardConfig } from '@/server/guardrails'

const Body = z.object({
  mode: z.enum(['off', 'observe', 'annotate', 'strict']),
  checks: z.record(z.string(), z.boolean()),
  minConfidence: z.number().min(0).max(1),
  policedHosts: z.array(z.string().max(200)).max(100),
  coach: z.boolean().default(false),
})

// Confab guardrail config + observability (admin). GET → config + stats + recent
// findings. PUT → update config.
export const Route = createFileRoute('/api/admin/guardrails')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const [config, stats, findings] = await Promise.all([getGuardConfig(), guardStats(), listFindings(50)])
        return json({ config, stats, findings, rules: guardRuleMeta() })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await setGuardConfig(parsed.data)
        void logAudit({ actor: user.email ?? user.name ?? 'admin', action: 'settings.guardrails', targetType: 'settings', after: { mode: parsed.data.mode } })
        return json({ config: parsed.data })
      },
    },
  },
})
