import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { deleteCollectionById, getCollection, setBindings } from '@/server/retrieval/collections'
import { logAudit } from '@/server/audit'

const Body = z.object({
  bindings: z.array(z.object({ principalType: z.enum(['all', 'user', 'agent', 'team']), principalId: z.string().max(200).nullish() })).max(200),
})

// One collection (admin). PUT → set its access bindings. DELETE → drop it (auto
// collections are protected).
export const Route = createFileRoute('/api/rag/collections/$id')({
  server: {
    handlers: {
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if (!(await getCollection(params.id))) return json({ error: 'not found' }, { status: 404 })
        await setBindings(params.id, parsed.data.bindings.map((b) => ({ principalType: b.principalType, principalId: b.principalId ?? null })))
        void logAudit({ actor: user.email ?? user.name ?? 'admin', action: 'rag.bindings', targetType: 'rag_collection', targetId: params.id })
        return json({ ok: true })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const r = await deleteCollectionById(params.id)
        if (!r.ok) return json({ error: r.error }, { status: 400 })
        void logAudit({ actor: user.email ?? user.name ?? 'admin', action: 'rag.delete', targetType: 'rag_collection', targetId: params.id })
        return json({ ok: true })
      },
    },
  },
})
