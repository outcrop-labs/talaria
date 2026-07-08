import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { deleteAgentSecret, listAgentSecrets, setAgentSecret } from '@/server/agent-secrets'
import { ownsAgent } from '@/server/personal-agent'

const PutBody = z.object({
  name: z.string().trim().min(2).max(64),
  value: z.string().min(1).max(8192),
})

// Per-agent secrets, write-only. GET → names + timestamps (never values).
// PUT { name, value } → set/replace. DELETE ?name= → remove. Admin, or the
// owner of a personal assistant. Takes effect on the next start from Talaria.
export const Route = createFileRoute('/api/fleet/agents/$id/secrets')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        return json({ secrets: await listAgentSecrets(params.id) })
      },
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        const parsed = PutBody.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: parsed.error.issues[0]?.message ?? 'bad request' }, { status: 400 })
        try {
          await setAgentSecret(params.id, parsed.data.name, parsed.data.value, user.email ?? user.name ?? null)
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        const name = new URL(request.url).searchParams.get('name')
        if (!name) return json({ error: 'missing name' }, { status: 400 })
        await deleteAgentSecret(params.id, name)
        return json({ ok: true })
      },
    },
  },
})
