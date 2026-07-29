import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { deleteComment, setResolved } from '@/server/kb-comments'

// One comment. PATCH { resolved } → resolve/unresolve its thread (author,
// thread starter, or doc owner). DELETE → remove your own comment.
export const Route = createFileRoute('/api/kb/comments/$id')({
  server: {
    handlers: {
      PATCH: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z.object({ resolved: z.boolean() }).safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const ok = await setResolved(params.id, parsed.data.resolved, user.id)
        return ok ? json({ ok: true }) : json({ error: 'forbidden' }, { status: 403 })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const ok = await deleteComment(params.id, user.id)
        return ok ? json({ ok: true }) : json({ error: 'forbidden' }, { status: 403 })
      },
    },
  },
})
