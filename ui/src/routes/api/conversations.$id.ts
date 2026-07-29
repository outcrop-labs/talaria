import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { getConversation } from '@/server/conversations'
import { db } from '@/server/db/pg'

// GET /api/conversations/:id → a conversation + its messages (ownership-checked).
// PATCH { title } → rename (owner, or a plan collaborator). A renamed title no
// longer matches the mechanical first-message truncation, so the Titler and
// its sweep leave it alone from then on.
export const Route = createFileRoute('/api/conversations/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const result = await getConversation(user.id, params.id)
        if (!result) return json({ error: 'not found' }, { status: 404 })
        return json(result)
      },
      PATCH: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await getConversation(user.id, params.id))) return json({ error: 'not found' }, { status: 404 })
        const parsed = z
          .object({ title: z.string().trim().min(1).max(120) })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const sql = await db()
        await sql`update conversations set title = ${parsed.data.title} where id = ${params.id}`
        return json({ ok: true })
      },
    },
  },
})
