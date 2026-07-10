import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
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
export const Route = createFileRoute('/api/rag/collections')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        await ensureAutoCollections().catch(() => {})
        return json({ collections: await listCollections() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          const col = await createCollection({
            name: parsed.data.name,
            description: parsed.data.description,
            createdBy: user.email ?? user.name ?? 'admin',
            bindings: parsed.data.bindings?.map((b) => ({ principalType: b.principalType, principalId: b.principalId ?? null })),
          })
          void logAudit({ actor: user.email ?? user.name ?? 'admin', action: 'rag.create', targetType: 'rag_collection', targetId: col.id, targetLabel: col.name })
          return json({ collection: col })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
