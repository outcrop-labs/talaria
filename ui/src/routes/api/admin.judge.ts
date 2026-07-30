import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { getJudgeConfig, setJudgeConfig } from '@/server/judge'
import { gatewayModels } from '@/server/llm-gateway'

// The automated QA judge config (admin). GET → current + available models.
// PUT → enable/disable + pick the judge model.
export const Route = createFileRoute('/api/admin/judge')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireAdmin(request)
        if (gate instanceof Response) return gate
        const [config, models] = await Promise.all([getJudgeConfig(), gatewayModels().catch(() => [])])
        return json({ config, models: models.map((m) => m.id) })
      },
      PUT: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ enabled: z.boolean(), model: z.string().max(200).nullish(), mode: z.enum(['advisory', 'enforcing']).optional() }))
        if (body instanceof Response) return body
        const config = { enabled: body.enabled, model: body.model?.trim() || null, mode: body.mode ?? 'enforcing' }
        await setJudgeConfig(config)
        void logAudit({ actor: actorOf(user), action: 'settings.judge', targetType: 'settings', after: config })
        return json({ config })
      },
    },
  },
})
