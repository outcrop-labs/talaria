import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireUser } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { moveDoc } from '@/server/kb'

const Body = z.object({
  parentId: z.string().uuid().nullable(),
  sort: z.number().int().default(0),
})

// Reparent / reorder a doc in the sidebar tree. Rejects cycles server-side.
export const Route = defineApi('/api/kb/docs/$id/move', {
  POST: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const doc = await moveDoc(params.id, body.parentId, body.sort)
    if (!doc) return json({ error: 'invalid move' }, { status: 400 })
    void logAudit({ actor: actorOf(user), action: 'kb.doc_move', targetType: 'kb-doc', targetId: params.id, targetLabel: doc.title, after: { parentId: body.parentId, sort: body.sort } })
    return json({ doc })
  },
})
