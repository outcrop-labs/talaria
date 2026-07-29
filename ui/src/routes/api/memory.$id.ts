import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { readMemory, writeMemory } from '@/server/agent-memory'
import { ownsAgent } from '@/server/personal-agent'

const Body = z.object({ content: z.string().max(2_000_000) })

// One managed agent's MEMORY.md, read/written through its running container.
// Writes: admin, or the owner of a personal assistant for its own memory.
export const Route = createFileRoute('/api/memory/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
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
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: params.id })))
          return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        try {
          await writeMemory(params.id, body.content, user.email ?? user.name ?? 'admin')
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
