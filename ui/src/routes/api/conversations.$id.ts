import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { getConversation } from '@/server/conversations'
import { db } from '@/server/db/pg'

// GET /api/conversations/:id → a conversation + its messages (ownership-checked).
// PATCH { title } → rename (owner, or a plan collaborator). A renamed title no
// longer matches the mechanical first-message truncation, so the Titler and
// its sweep leave it alone from then on.
export const Route = defineApi('/api/conversations/$id', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const result = await getConversation(user.id, params.id)
    if (!result) return json({ error: 'not found' }, { status: 404 })
    return json(result)
  },
  PATCH: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await getConversation(user.id, params.id))) return json({ error: 'not found' }, { status: 404 })
    const body = await parseBody(request, z.object({ title: z.string().trim().min(1).max(120) }))
    if (body instanceof Response) return body
    const sql = await db()
    await sql`update conversations set title = ${body.title} where id = ${params.id}`
    return json({ ok: true })
  },
})
