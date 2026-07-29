import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
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
export const Route = createFileRoute('/api/admin/outreach')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const sql = await db()
        const agents = (await sql`
          select model, display_name as "displayName", proactive, owner_user_id is not null as personal
          from agent_defs where enabled order by slug
        `) as unknown as Array<{ model: string; displayName: string; proactive: boolean; personal: boolean }>
        return json({ config: await getOutreachConfig(), agents, events: await recentOutreachEvents() })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const { proactiveAgents, ...config } = parsed.data
        await setOutreachConfig(config)
        const sql = await db()
        await sql`update agent_defs set proactive = (model = any(${proactiveAgents})) where enabled`
        void logAudit({
          actor: user.email ?? user.name ?? 'admin',
          action: 'outreach.config',
          targetType: 'outreach',
          after: { proactiveAgents },
        })
        return json({ ok: true })
      },
    },
  },
})
