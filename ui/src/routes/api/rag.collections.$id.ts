import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { deleteCollectionById, getCollection, setBindings } from '@/server/retrieval/collections'
import { logAudit } from '@/server/audit'

const Body = z.object({
  bindings: z.array(z.object({ principalType: z.enum(['all', 'user', 'agent', 'team']), principalId: z.string().max(200).nullish() })).max(200),
})

// One collection (admin). PUT → set its access bindings. DELETE → drop it (auto
// collections are protected).
export const Route = defineApi('/api/rag/collections/$id', {
  PUT: async ({ request, params }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    if (!(await getCollection(params.id))) return json({ error: 'not found' }, { status: 404 })
    await setBindings(params.id, body.bindings.map((b) => ({ principalType: b.principalType, principalId: b.principalId ?? null })))
    void logAudit({ actor: actorOf(user), action: 'rag.bindings', targetType: 'rag_collection', targetId: params.id })
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const r = await deleteCollectionById(params.id)
    if (!r.ok) return json({ error: r.error }, { status: 400 })
    void logAudit({ actor: actorOf(user), action: 'rag.delete', targetType: 'rag_collection', targetId: params.id })
    return json({ ok: true })
  },
})
