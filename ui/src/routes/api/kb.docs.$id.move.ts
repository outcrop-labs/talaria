import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { actorOf, parseBody, requirePerm } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { effectiveDocPerms, getDoc, moveDoc } from '@/server/kb'
import { canEditHuman } from '@/server/kb-perms'

const Body = z.object({
  parentId: Uuid.nullable(),
  sort: z.number().int().default(0),
})

// Reparent / reorder a doc in the sidebar tree. Rejects cycles server-side.
// Moving a doc is an edit of it, so it takes the same gate the PUT does —
// otherwise any signed-in member could reparent a private doc out of a folder
// they can't even read. `moveDoc` itself only detects cycles and always has.
export const Route = defineApi('/api/kb/docs/$id/move', {
  POST: async ({ request, params }) => {
    const existing = await getDoc(params.id)
    if (!existing) return json({ error: 'not found' }, { status: 404 })
    const gate = await requirePerm(request, 'kb.edit')
    if (gate instanceof Response) return gate
    const user = gate
    const { perms, grants } = await effectiveDocPerms(existing)
    if (!canEditHuman(perms, user.id, user.email ?? user.name, grants)) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    // The tree is per-space: a cross-space parent would drag the doc under a
    // folder whose audience never vetted it.
    if (body.parentId) {
      const parent = await getDoc(body.parentId)
      if (!parent || parent.spaceId !== existing.spaceId) return json({ error: 'parent must live in the same space' }, { status: 400 })
    }
    const doc = await moveDoc(params.id, body.parentId, body.sort)
    if (!doc) return json({ error: 'invalid move' }, { status: 400 })
    void logAudit({ actor: actorOf(user), action: 'kb.doc_move', targetType: 'kb-doc', targetId: params.id, targetLabel: doc.title, after: { parentId: body.parentId, sort: body.sort } })
    return json({ doc })
  },
})
