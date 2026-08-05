import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { addComment, canDiscussDoc, listComments } from '@/server/kb-comments'

// Doc comment threads. GET → all comments (client assembles threads).
// POST { content, parentId?, quote? } → comment/reply. Read access to the doc
// is the gate for both — discussion is part of the document.
export const Route = defineApi('/api/kb/docs/$id/comments', {
  GET: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    if (!(await canDiscussDoc(params.id, user.id, user.email ?? user.name))) return json({ error: 'not found' }, { status: 404 })
    return json({ comments: await listComments(params.id) })
  },
  POST: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    if (!(await canDiscussDoc(params.id, user.id, user.email ?? user.name))) return json({ error: 'not found' }, { status: 404 })
    const body = await parseBody(
      request,
      z.object({
        content: z.string().trim().min(1).max(8_000),
        parentId: z.string().uuid().nullish(),
        quote: z.string().trim().max(500).nullish(),
      }),
    )
    if (body instanceof Response) return body
    const comment = await addComment({
      docId: params.id,
      parentId: body.parentId ?? null,
      authorUserId: user.id,
      author: user.name ?? user.email ?? 'user',
      quote: body.quote ?? null,
      content: body.content,
    })
    return json({ comment })
  },
})
