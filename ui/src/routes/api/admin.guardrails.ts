import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
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
        const gate = await requireAdmin(request)
        if (gate instanceof Response) return gate
        const [config, stats, findings] = await Promise.all([getGuardConfig(), guardStats(), listFindings(50)])
        return json({ config, stats, findings, rules: guardRuleMeta() })
      },
      PUT: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        await setGuardConfig(body)
        void logAudit({ actor: actorOf(user), action: 'settings.guardrails', targetType: 'settings', after: { mode: body.mode } })
        return json({ config: body })
      },
    },
  },
})
