import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { readMemory, writeMemory } from '@/server/agent-memory'
import { ownsAgent } from '@/server/personal-agent'

const Body = z.object({ content: z.string().max(2_000_000) })

// One managed agent's MEMORY.md, read/written through its running container.
// Writes: admin, or the owner of a personal assistant for its own memory.
export const Route = createFileRoute('/api/memory/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        // Same gate as PUT: an agent's memory is its owner's (or admin's)
        // business — a personal assistant's memory is dense private context.
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        try {
          return json(await readMemory(params.id))
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          await writeMemory(params.id, parsed.data.content, user.email ?? user.name ?? 'admin')
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
