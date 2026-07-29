import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { addComment, canDiscussDoc, listComments } from '@/server/kb-comments'

// Doc comment threads. GET → all comments (client assembles threads).
// POST { content, parentId?, quote? } → comment/reply. Read access to the doc
// is the gate for both — discussion is part of the document.
export const Route = createFileRoute('/api/kb/docs/$id/comments')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await canDiscussDoc(params.id, user.id, user.email ?? user.name))) return json({ error: 'not found' }, { status: 404 })
        return json({ comments: await listComments(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await canDiscussDoc(params.id, user.id, user.email ?? user.name))) return json({ error: 'not found' }, { status: 404 })
        const parsed = z
          .object({
            content: z.string().trim().min(1).max(8_000),
            parentId: z.string().uuid().nullish(),
            quote: z.string().trim().max(500).nullish(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const comment = await addComment({
          docId: params.id,
          parentId: parsed.data.parentId ?? null,
          authorUserId: user.id,
          author: user.name ?? user.email ?? 'user',
          quote: parsed.data.quote ?? null,
          content: parsed.data.content,
        })
        return json({ comment })
      },
    },
  },
})
