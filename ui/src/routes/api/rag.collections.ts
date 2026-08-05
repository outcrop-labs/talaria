import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin, requireUser } from '@/server/api-guard'
import { createCollection, ensureAutoCollections, listCollections } from '@/server/retrieval/collections'
import { logAudit } from '@/server/audit'

const Binding = z.object({
  principalType: z.enum(['all', 'user', 'agent', 'team']),
  principalId: z.string().max(200).nullish(),
})
const Body = z.object({
  name: z.string().min(2).max(80),
  description: z.string().max(500).optional(),
  bindings: z.array(Binding).max(200).optional(),
})

// The RAG collection registry (admin). GET → all collections + bindings (the two
// auto ones are ensured first). POST → spin up a new custom collection.
export const Route = defineApi('/api/rag/collections', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    await ensureAutoCollections().catch(() => {})
    const collections = await listCollections()
    // Members get names only (the doc "Brain" picker); the binding matrix
    // is admin governance.
    if (user.role !== 'admin') {
      return json({ collections: collections.map(({ id, name, kind, description, auto }) => ({ id, name, kind, description, auto, bindings: [] })) })
    }
    return json({ collections })
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    try {
      const col = await createCollection({
        name: body.name,
        description: body.description,
        createdBy: user.email ?? user.name ?? 'admin',
        bindings: body.bindings?.map((b) => ({ principalType: b.principalType, principalId: b.principalId ?? null })),
      })
      void logAudit({ actor: actorOf(user), action: 'rag.create', targetType: 'rag_collection', targetId: col.id, targetLabel: col.name })
      return json({ collection: col })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
})
