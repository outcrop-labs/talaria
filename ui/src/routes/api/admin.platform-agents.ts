import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { PLATFORM_AGENTS, getPlatformAgentModels, setPlatformAgentModel, type PlatformAgentId } from '@/server/platform-agents'
import { getJudgeConfig, setJudgeConfig } from '@/server/judge'
import { agentSlot, getEffortPrefs, setEffortPref } from '@/server/effort-prefs'
import { effortsForModel } from '@/server/model-efforts'
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
    const prefs = await getEffortPrefs()
    const efforts = Object.fromEntries(IDS.map((id) => [id, prefs[agentSlot(id)] ?? null]))
    return json({
      agents: PLATFORM_AGENTS,
      assignments,
      models: (await gatewayModels()).map((m) => m.id),
      // The per-agent effort preference (null = the model's own default).
      efforts,
    })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({
        id: z.enum(IDS as [PlatformAgentId, ...PlatformAgentId[]]),
        model: z.string().max(200).nullable().optional(),
        // The agent's effort preference. Absent = leave it alone; null = clear.
        effort: z.string().min(1).max(24).nullable().optional(),
      }),
    )
    if (body instanceof Response) return body
    if (body.model && !(await gatewayModels()).some((m) => m.id === body.model)) {
      return json({ error: 'that model is not on the gateway' }, { status: 400 })
    }
    if (body.effort !== undefined) {
      // Same rule as the roles route: validated against the assigned model's
      // published levels, and refused on Auto where there is nothing to ride.
      const current = body.id === 'judge' ? (await getJudgeConfig()).model : (await getPlatformAgentModels())[body.id]
      const target = body.model !== undefined && body.model !== null ? body.model : current ?? null
      if (body.effort && !target) return json({ error: 'assign a model before setting its effort' }, { status: 400 })
      if (body.effort && target && !(await effortsForModel(target)).includes(body.effort)) {
        return json({ error: `that model does not publish the "${body.effort}" effort level` }, { status: 400 })
      }
      await setEffortPref(agentSlot(body.id), body.effort)
      void logAudit({
        actor: actorOf(user),
        action: 'platform_agent.effort',
        targetType: 'platform-agent',
        targetId: body.id,
        after: { effort: body.effort },
      })
    }
    if (body.model !== undefined) {
      if (body.id === 'judge') {
        const cfg = await getJudgeConfig()
        await setJudgeConfig({ ...cfg, model: body.model })
      } else {
        await setPlatformAgentModel(body.id, body.model)
      }
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
