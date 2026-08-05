import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
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
export const Route = defineApi('/api/admin/platform-agents', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const assignments: Partial<Record<string, string>> = { ...(await getPlatformAgentModels()) }
    assignments.judge = (await getJudgeConfig()).model ?? undefined
    return json({
      agents: PLATFORM_AGENTS,
      assignments,
      models: (await gatewayModels()).map((m) => m.id),
    })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({ id: z.enum(IDS as [PlatformAgentId, ...PlatformAgentId[]]), model: z.string().max(200).nullable() }),
    )
    if (body instanceof Response) return body
    if (body.model && !(await gatewayModels()).some((m) => m.id === body.model)) {
      return json({ error: 'that model is not on the gateway' }, { status: 400 })
    }
    if (body.id === 'judge') {
      const cfg = await getJudgeConfig()
      await setJudgeConfig({ ...cfg, model: body.model })
    } else {
      await setPlatformAgentModel(body.id, body.model)
    }
    void logAudit({
      actor: actorOf(user),
      action: 'platform_agent.assign',
      targetType: 'platform-agent',
      targetId: body.id,
      after: { model: body.model },
    })
    return json({ ok: true })
  },
})
