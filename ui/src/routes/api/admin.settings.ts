import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { auditRetentionDays, logAudit, setSetting } from '@/server/audit'
import { orgProfile, setOrgProfile } from '@/server/org'
import { memberModelAllowlist, setMemberModelAllowlist } from '@/server/model-access'
import { rollRunningAgents } from '@/server/fleet-reconcile'
import { getBudgets, setBudgets } from '@/server/llm-gateway'
import { cronFloorMinutes } from '@/server/agent-crons'

// App settings (admin). GET → current values. PUT → update. Grows as more
// app-wide settings land; audit retention is the first.

/** Null or absent = unlimited. Zero is treated as unlimited too, so a cleared
 *  field can never accidentally mean "refuse everything". */
const budgetLimits = z
  .object({
    tokens: z.number().int().min(0).max(1e12).nullable().optional(),
    usd: z.number().min(0).max(1e9).nullable().optional(),
  })
  .nullable()
export const Route = defineApi('/api/admin/settings', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    return json({
      auditRetentionDays: await auditRetentionDays(),
      org: await orgProfile(),
      memberModels: await memberModelAllowlist(),
      llmBudgets: await getBudgets(),
      cronMinIntervalMinutes: await cronFloorMinutes(),
    })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({
        auditRetentionDays: z.number().int().min(0).max(3650).optional(),
        org: z.object({ name: z.string().max(120).optional(), about: z.string().max(2000).optional() }).optional(),
        /** Bare model ids members may pick; empty = all models. */
        memberModels: z.array(z.string().min(1).max(200)).max(200).optional(),
        /** Rolling-window LLM spend ceilings. All-null = unlimited (the
         *  default), which is what every existing deployment gets. */
        llmBudgets: z
          .object({
            windowHours: z.number().int().min(1).max(8760),
            org: budgetLimits.optional().default(null),
            perAgent: budgetLimits.optional().default(null),
            agents: z.record(z.string().min(1).max(200), budgetLimits).optional().default({}),
          })
          .optional(),
        /** Fastest cron an agent may be given, in minutes. 0 = no floor. */
        cronMinIntervalMinutes: z.number().int().min(0).max(1440).optional(),
      }),
    )
    if (body instanceof Response) return body
    if (body.llmBudgets !== undefined) {
      const before = await getBudgets()
      await setBudgets({
        windowHours: body.llmBudgets.windowHours,
        org: body.llmBudgets.org ?? null,
        perAgent: body.llmBudgets.perAgent ?? null,
        agents: Object.fromEntries(
          Object.entries(body.llmBudgets.agents ?? {}).filter(([, v]) => v !== null),
        ) as Record<string, { tokens?: number | null; usd?: number | null }>,
      })
      // A spend ceiling is a governance control: who moved it, and from what.
      void logAudit({
        actor: actorOf(user),
        action: 'settings.llm_budgets',
        targetType: 'settings',
        before,
        after: body.llmBudgets,
      })
    }
    if (body.cronMinIntervalMinutes !== undefined) {
      await setSetting('cron_min_interval_minutes', body.cronMinIntervalMinutes)
      void logAudit({
        actor: actorOf(user),
        action: 'settings.cron_min_interval',
        targetType: 'settings',
        after: { cronMinIntervalMinutes: body.cronMinIntervalMinutes },
      })
    }
    if (body.auditRetentionDays !== undefined) {
      await setSetting('audit_retention_days', body.auditRetentionDays)
      void logAudit({
        actor: actorOf(user),
        action: 'settings.audit_retention',
        targetType: 'settings',
        after: { auditRetentionDays: body.auditRetentionDays },
      })
    }
    if (body.memberModels !== undefined) {
      await setMemberModelAllowlist(body.memberModels)
      void logAudit({
        actor: actorOf(user),
        action: 'settings.member_models',
        targetType: 'settings',
        after: { memberModels: body.memberModels },
      })
    }
    if (body.org) {
      await setOrgProfile(body.org)
      // The org lives in every rendered soul — propagate by ROLLING running
      // agents (new container up + healthy before the old one retires), so
      // an identity edit never kills anyone's in-flight conversation.
      void rollRunningAgents().catch(() => {})
      void logAudit({
        actor: actorOf(user),
        action: 'settings.org',
        targetType: 'settings',
        after: body.org,
      })
    }
    return json({ ok: true })
  },
})
