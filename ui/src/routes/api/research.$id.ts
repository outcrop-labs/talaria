import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { checkAgentKey } from '@/server/agent-auth'
import { getResearchRun } from '@/server/research'
import { db } from '@/server/db/pg'

// GET → one run + its citation registry (org-visible). DELETE → owner/admin.
export const Route = createFileRoute('/api/research/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        if (!checkAgentKey(request) && !(await getSessionUser(request))) {
          return json({ error: 'unauthorized' }, { status: 401 })
        }
        const result = await getResearchRun(params.id)
        if (!result) return json({ error: 'not found' }, { status: 404 })
        return json(result)
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const result = await getResearchRun(params.id)
        if (!result) return json({ error: 'not found' }, { status: 404 })
        if (result.run.ownerUserId !== user.id && user.role !== 'admin') {
          return json({ error: 'forbidden' }, { status: 403 })
        }
        // The report artifact survives — deleting a run clears the queue entry,
        // not the knowledge.
        const sql = await db()
        await sql`delete from research_runs where id = ${params.id}`
        return json({ ok: true })
      },
    },
  },
})
