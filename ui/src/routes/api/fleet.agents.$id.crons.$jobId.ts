import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { editCronJob, pauseCronJob, removeCronJob, resumeCronJob, runCronJob } from '@/server/agent-crons'
import { ownsAgent } from '@/server/personal-agent'
import { logAudit } from '@/server/audit'

const Body = z.object({ action: z.enum(['pause', 'resume', 'run']) })
const EditBody = z.object({
  name: z.string().min(1).max(80).optional(),
  schedule: z.string().min(1).max(120).optional(),
  prompt: z.string().min(1).max(20_000).optional(),
})

// One cron job: DELETE → remove. POST { action } → pause | resume | run
// ("run" queues it for the next scheduler tick, ≤60s). PUT { name? schedule?
// prompt? } → edit in place. Admin or owner.
export const Route = defineApi('/api/fleet/agents/$id/crons/$jobId', {
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await hasPerm(user, 'agents.manage')) && !(await ownsAgent(user.id, { defId: params.id })))
      return json({ error: 'forbidden' }, { status: 403 })
    try {
      await removeCronJob(params.id, params.jobId)
      void logAudit({ actor: actorOf(user), action: 'cron.delete', targetType: 'agent', targetId: params.id, after: { jobId: params.jobId } })
      return json({ ok: true })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  PUT: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await hasPerm(user, 'agents.manage')) && !(await ownsAgent(user.id, { defId: params.id })))
      return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, EditBody)
    if (body instanceof Response) return body
    try {
      await editCronJob(params.id, params.jobId, body)
      void logAudit({ actor: actorOf(user), action: 'cron.update', targetType: 'agent', targetId: params.id, after: { jobId: params.jobId, name: body.name, schedule: body.schedule } })
      return json({ ok: true })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await hasPerm(user, 'agents.manage')) && !(await ownsAgent(user.id, { defId: params.id })))
      return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    try {
      const fn = { pause: pauseCronJob, resume: resumeCronJob, run: runCronJob }[body.action]
      await fn(params.id, params.jobId)
      void logAudit({ actor: actorOf(user), action: `cron.${body.action}`, targetType: 'agent', targetId: params.id, after: { jobId: params.jobId } })
      return json({ ok: true })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
})
