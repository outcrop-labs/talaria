import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { PLATFORM_AGENTS, getPlatformAgentModels, setPlatformAgentModel, type PlatformAgentId } from '@/server/platform-agents'
import { getJudgeConfig, setJudgeConfig } from '@/server/judge'
import { gatewayModels } from '@/server/llm-gateway'
import { logAudit } from '@/server/audit'

const IDS = PLATFORM_AGENTS.filter((a) => a.assignable).map((a) => a.id)

// Platform sub-agents — Talaria's own workers (Muse, Distiller, Concluder, )
// and which model powers each. GET → registry + assignments + assignable
// models. PUT { id, model|null } → assign (null = back to auto). The Judge's
// pick lives in its own judge_config (shared with the Guard panel) — this
// route reads/writes it there so there's one source of truth. Admins only.
export const Route = createFileRoute('/api/admin/platform-agents')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const assignments: Partial<Record<string, string>> = { ...(await getPlatformAgentModels()) }
        assignments.judge = (await getJudgeConfig()).model ?? undefined
        return json({
          agents: PLATFORM_AGENTS,
          assignments,
          models: (await gatewayModels()).map((m) => m.id),
        })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ id: z.enum(IDS as [PlatformAgentId, ...PlatformAgentId[]]), model: z.string().max(200).nullable() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if (parsed.data.model && !(await gatewayModels()).some((m) => m.id === parsed.data.model)) {
          return json({ error: 'that model is not on the gateway' }, { status: 400 })
        }
        if (parsed.data.id === 'judge') {
          const cfg = await getJudgeConfig()
          await setJudgeConfig({ ...cfg, model: parsed.data.model })
        } else {
          await setPlatformAgentModel(parsed.data.id, parsed.data.model)
        }
        void logAudit({
          actor: user.email ?? user.name ?? 'admin',
          action: 'platform_agent.assign',
          targetType: 'platform-agent',
          targetId: parsed.data.id,
          after: { model: parsed.data.model },
        })
        return json({ ok: true })
      },
    },
  },
})
