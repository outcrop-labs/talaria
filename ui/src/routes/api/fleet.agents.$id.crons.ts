import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { createCronJob, listCronJobs } from '@/server/agent-crons'
import { ownsAgent } from '@/server/personal-agent'
import { logAudit } from '@/server/audit'

const Body = z.object({
  name: z.string().trim().min(1).max(80),
  schedule: z.string().trim().min(1).max(120),
  prompt: z.string().trim().min(1).max(20_000),
})

// One agent's native Hermes cron jobs. GET → jobs (read from the container's
// jobs.json). POST → create. Admin, or the owner of a personal assistant.
export const Route = defineApi('/api/fleet/agents/$id/crons', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await hasPerm(user, 'agents.manage')) && !(await ownsAgent(user.id, { defId: params.id })))
      return json({ error: 'forbidden' }, { status: 403 })
    try {
      return json({ jobs: await listCronJobs(params.id) })
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
      const created = await createCronJob(params.id, body)
      void logAudit({ actor: actorOf(user), action: 'cron.create', targetType: 'agent', targetId: params.id, after: { name: body.name, schedule: body.schedule } })
      return json({ ok: true, ...created })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
})
