import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createFleetCrons, listFleetCrons } from '@/server/agent-crons'

const Body = z.object({
  agentIds: z.array(z.string().uuid()).min(1).max(64),
  name: z.string().trim().min(1).max(80),
  schedule: z.string().trim().min(1).max(120),
  prompt: z.string().trim().min(1).max(20_000),
  staggerMinutes: z.number().int().min(0).max(30).optional(),
})

// Fleet-wide crons (admin). GET → every managed agent's jobs (down containers
// reported per-agent, not fatal). POST → create the same job across agents,
// staggered per agent when the schedule is a fixed-minute cron expression.
export const Route = createFileRoute('/api/fleet/crons')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ agents: await listFleetCrons() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: parsed.error.issues[0]?.message ?? 'bad request' }, { status: 400 })
        return json({ results: await createFleetCrons(parsed.data) })
      },
    },
  },
})
