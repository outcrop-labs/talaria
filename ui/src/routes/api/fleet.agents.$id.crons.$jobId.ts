import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { editCronJob, pauseCronJob, removeCronJob, resumeCronJob, runCronJob } from '@/server/agent-crons'
import { ownsAgent } from '@/server/personal-agent'

const Body = z.object({ action: z.enum(['pause', 'resume', 'run']) })
const EditBody = z.object({
  name: z.string().min(1).max(80).optional(),
  schedule: z.string().min(1).max(120).optional(),
  prompt: z.string().min(1).max(20_000).optional(),
})

// One cron job: DELETE → remove. POST { action } → pause | resume | run
// ("run" queues it for the next scheduler tick, ≤60s). PUT { name? schedule?
// prompt? } → edit in place. Admin or owner.
export const Route = createFileRoute('/api/fleet/agents/$id/crons/$jobId')({
  server: {
    handlers: {
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        try {
          await removeCronJob(params.id, params.jobId)
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        const parsed = EditBody.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          await editCronJob(params.id, params.jobId, parsed.data)
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          const fn = { pause: pauseCronJob, resume: resumeCronJob, run: runCronJob }[parsed.data.action]
          await fn(params.id, params.jobId)
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
