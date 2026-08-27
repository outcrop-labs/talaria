import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { getBacklinks, getDoc, effectiveDocPerms } from '@/server/kb'
import { canRead } from '@/server/kb-perms'

// Docs that link to this one ("linked from"). Editor links point at
// /knowledge/<id>, so backlinks fall out of a substring match. Gated by the
// SAME per-doc ACL as reading the doc — backlink titles leak content.
export const Route = defineApi('/api/kb/docs/$id/backlinks', {
  GET: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const doc = await getDoc(params.id)
    if (!doc) return json({ error: 'not found' }, { status: 404 })
    const { perms, grants } = await effectiveDocPerms(doc)
    if (!canRead(perms, user.id, user.email ?? user.name, grants)) return json({ error: 'forbidden' }, { status: 403 })
    return json({ backlinks: await getBacklinks(params.id) })
  },
})
