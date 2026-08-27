import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { createFleetCrons, listFleetCrons } from '@/server/agent-crons'
import { logAudit } from '@/server/audit'

const Body = z.object({
  agentIds: z.array(Uuid).min(1).max(64),
  name: z.string().trim().min(1).max(80),
  schedule: z.string().trim().min(1).max(120),
  prompt: z.string().trim().min(1).max(20_000),
  staggerMinutes: z.number().int().min(0).max(30).optional(),
})

// Fleet-wide crons (admin). GET → every managed agent's jobs (down containers
// reported per-agent, not fatal). POST → create the same job across agents,
// staggered per agent when the schedule is a fixed-minute cron expression.
export const Route = defineApi('/api/fleet/crons', {
  GET: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    return json({ agents: await listFleetCrons() })
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const results = await createFleetCrons(body)
    void logAudit({
      actor: actorOf(user),
      action: 'cron.create',
      targetType: 'fleet',
      targetId: 'fleet',
      targetLabel: body.name,
      after: { agentIds: body.agentIds, schedule: body.schedule },
    })
    return json({ results })
  },
})
