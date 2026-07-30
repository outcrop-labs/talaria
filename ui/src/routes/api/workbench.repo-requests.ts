import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { createRepo, setGrantedRepos, grantedRepos } from '@/server/github'
import { db } from '@/server/db/pg'
import { logAudit } from '@/server/audit'

const ROW = `id, agent_id as "agentId", agent_model as "agentModel", org, name, description, why,
  task_id as "taskId", status, decided_by as "decidedBy", created_at as "createdAt"`

// Agent repo-creation requests. GET → pending queue; PUT → approve (creates
// the repo via the App, auto-grants it to the requester) or reject. Admin —
// approval mints real org resources.
export const Route = createFileRoute('/api/workbench/repo-requests')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const sql = await db()
        const requests = await sql.unsafe(`select ${ROW} from workbench_repo_requests where status = 'pending' order by created_at`)
        return json({ requests })
      },
      PUT: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ id: z.string().uuid(), action: z.enum(['approve', 'reject']) }))
        if (body instanceof Response) return body
        const sql = await db()
        const [req] = (await sql.unsafe(`select ${ROW} from workbench_repo_requests where id = $1 and status = 'pending'`, [body.id])) as unknown as Array<{
          id: string
          agentId: string
          agentModel: string
          org: string
          name: string
          description: string
          taskId: string | null
        }>
        if (!req) return json({ error: 'not found or already decided' }, { status: 404 })
        const actor = actorOf(user)
        if (body.action === 'reject') {
          await sql`update workbench_repo_requests set status = 'rejected', decided_by = ${actor}, updated_at = now() where id = ${req.id}`
          return json({ ok: true })
        }
        try {
          const repo = await createRepo(req.org, req.name, req.description)
          // The requester gets the repo granted the moment it exists.
          await setGrantedRepos(req.agentId, [...new Set([...(await grantedRepos(req.agentId)), repo.fullName])])
          await sql`update workbench_repo_requests set status = 'approved', decided_by = ${actor}, updated_at = now() where id = ${req.id}`
          if (req.taskId) {
            const { logActivity } = await import('@/server/tasks')
            await logActivity(req.taskId, actor, 'workbench', `approved new repo ${repo.fullName} — created and granted to ${req.agentModel}`).catch(() => {})
          }
          void logAudit({ actor, action: 'workbench.repo_create', targetType: 'workbench', targetId: repo.fullName })
          return json({ ok: true, repo: repo.fullName, url: repo.url })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
