import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { logAudit } from '@/server/audit'
import { getJudgeConfig, setJudgeConfig } from '@/server/judge'
import { gatewayModels } from '@/server/llm-gateway'

// The automated QA judge config (admin). GET → current + available models.
// PUT → enable/disable + pick the judge model.
export const Route = createFileRoute('/api/admin/judge')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const [config, models] = await Promise.all([getJudgeConfig(), gatewayModels().catch(() => [])])
        return json({ config, models: models.map((m) => m.id) })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ enabled: z.boolean(), model: z.string().max(200).nullish() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const config = { enabled: parsed.data.enabled, model: parsed.data.model?.trim() || null }
        await setJudgeConfig(config)
        void logAudit({ actor: user.email ?? user.name ?? 'admin', action: 'settings.judge', targetType: 'settings', after: config })
        return json({ config })
      },
    },
  },
})
