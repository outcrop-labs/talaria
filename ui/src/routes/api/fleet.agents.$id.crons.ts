import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { createCronJob, listCronJobs } from '@/server/agent-crons'
import { ownsAgent } from '@/server/personal-agent'

const Body = z.object({
  name: z.string().trim().min(1).max(80),
  schedule: z.string().trim().min(1).max(120),
  prompt: z.string().trim().min(1).max(20_000),
})

// One agent's native Hermes cron jobs. GET → jobs (read from the container's
// jobs.json). POST → create. Admin, or the owner of a personal assistant.
export const Route = createFileRoute('/api/fleet/agents/$id/crons')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'agents.manage')) && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        try {
          return json({ jobs: await listCronJobs(params.id) })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'agents.manage')) && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: parsed.error.issues[0]?.message ?? 'bad request' }, { status: 400 })
        try {
          return json({ ok: true, ...(await createCronJob(params.id, parsed.data)) })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
