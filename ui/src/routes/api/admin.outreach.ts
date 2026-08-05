import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { db } from '@/server/db/pg'
import { getOutreachConfig, recentOutreachEvents, setOutreachConfig } from '@/server/outreach'
import { logAudit } from '@/server/audit'

const Body = z.object({
  enabled: z.boolean(),
  intervalMinutes: z.number().int().min(15).max(24 * 60),
  dailyDmCap: z.number().int().min(1).max(20),
  /** Models of the agents allowed to be proactive. */
  proactiveAgents: z.array(z.string()).max(100),
})

// GET → config + per-agent proactive flags + recent events. PUT → save both.
// Admin-only; the sweep itself stays off unless `enabled`.
export const Route = defineApi('/api/admin/outreach', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const sql = await db()
    const agents = (await sql`
      select model, display_name as "displayName", proactive, owner_user_id is not null as personal
      from agent_defs where enabled order by slug
    `) as unknown as Array<{ model: string; displayName: string; proactive: boolean; personal: boolean }>
    return json({ config: await getOutreachConfig(), agents, events: await recentOutreachEvents() })
  },
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const { proactiveAgents, ...config } = body
    await setOutreachConfig(config)
    const sql = await db()
    await sql`update agent_defs set proactive = (model = any(${proactiveAgents})) where enabled`
    void logAudit({
      actor: actorOf(user),
      action: 'outreach.config',
      targetType: 'outreach',
      after: { proactiveAgents },
    })
    return json({ ok: true })
  },
})
