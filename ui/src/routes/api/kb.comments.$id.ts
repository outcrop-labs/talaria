import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { deleteComment, setResolved } from '@/server/kb-comments'

// One comment. PATCH { resolved } → resolve/unresolve its thread (author,
// thread starter, or doc owner). DELETE → remove your own comment.
export const Route = defineApi('/api/kb/comments/$id', {
  PATCH: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const body = await parseBody(request, z.object({ resolved: z.boolean() }))
    if (body instanceof Response) return body
    const ok = await setResolved(params.id, body.resolved, user.id)
    return ok ? json({ ok: true }) : json({ error: 'forbidden' }, { status: 403 })
  },
  DELETE: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const ok = await deleteComment(params.id, user.id)
    return ok ? json({ ok: true }) : json({ error: 'forbidden' }, { status: 403 })
  },
})
