import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { moveDoc } from '@/server/kb'

const Body = z.object({
  parentId: z.string().uuid().nullable(),
  sort: z.number().int().default(0),
})

// Reparent / reorder a doc in the sidebar tree. Rejects cycles server-side.
export const Route = createFileRoute('/api/kb/docs/$id/move')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const doc = await moveDoc(params.id, parsed.data.parentId, parsed.data.sort)
        if (!doc) return json({ error: 'invalid move' }, { status: 400 })
        return json({ doc })
      },
    },
  },
})
