import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { getResearchRun, researchRole } from '@/server/research'
import { personalAssistantOwners } from '@/server/users'
import { db } from '@/server/db/pg'

// GET → one run + its citation registry (owner / shared member / org runs).
// DELETE → owner/admin.
export const Route = createFileRoute('/api/research/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        let viewer: string | null = null
        if (checkAgentKey(request)) {
          const name = agentName(request)
          viewer = name ? ((await personalAssistantOwners()).get(name) ?? null) : null
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          viewer = user.id
        }
        if (!(await researchRole(viewer, params.id))) return json({ error: 'not found' }, { status: 404 })
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
