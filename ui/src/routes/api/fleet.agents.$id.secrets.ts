import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { deleteAgentSecret, listAgentSecrets, setAgentSecret } from '@/server/agent-secrets'
import { ownsAgent } from '@/server/personal-agent'
import { logAudit } from '@/server/audit'

const PutBody = z.object({
  name: z.string().trim().min(2).max(64),
  value: z.string().min(1).max(8192),
})

// Per-agent secrets, write-only. GET → names + timestamps (never values).
// PUT { name, value } → set/replace. DELETE { name } → remove. Admin, or the
// owner of a personal assistant. Takes effect on the next start from Talaria.
// Every write audits — secret NAMES only, never values.
export const Route = defineApi('/api/fleet/agents/$id/secrets', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
      return json({ error: 'forbidden' }, { status: 403 })
    return json({ secrets: await listAgentSecrets(params.id) })
  },
  PUT: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
      return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, PutBody)
    if (body instanceof Response) return body
    try {
      await setAgentSecret(params.id, body.name, body.value, user.email ?? user.name ?? null)
      void logAudit({
        actor: actorOf(user),
        action: 'agent.secret_set',
        targetType: 'agent',
        targetId: params.id,
        after: { name: body.name },
      })
      return json({ ok: true })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
      return json({ error: 'forbidden' }, { status: 403 })
    // Body { name } — matches PUT's transport (was ?name=).
    const parsed = z
      .object({ name: z.string().min(1).max(64) })
      .safeParse(await request.json().catch(() => null))
    const name = parsed.success ? parsed.data.name : new URL(request.url).searchParams.get('name')
    if (!name) return json({ error: 'missing name' }, { status: 400 })
    await deleteAgentSecret(params.id, name)
    void logAudit({
      actor: actorOf(user),
      action: 'agent.secret_delete',
      targetType: 'agent',
      targetId: params.id,
      after: { name },
    })
    return json({ ok: true })
  },
})
