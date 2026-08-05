import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requirePerm } from '@/server/api-guard'
import { grantedRepos, listReachableRepos, setGrantedRepos } from '@/server/github'
import { db } from '@/server/db/pg'

async function agentExists(id: string): Promise<boolean> {
  const sql = await db()
  const rows = await sql`select 1 from agent_defs where id = ${id}::uuid`.catch(() => [])
  return rows.length > 0
}

// Per-agent workbench repo grants — explicit, like MCP assignment. GET →
// the connection's reachable pool + this agent's grants; PUT → replace the
// grant set (validated against the pool). agents.manage.
export const Route = defineApi('/api/workbench/repos/$agentId', {
  GET: async ({ request, params }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    if (!(await agentExists(params.agentId))) return json({ error: 'unknown agent' }, { status: 404 })
    const [available, granted] = await Promise.all([listReachableRepos(), grantedRepos(params.agentId)])
    return json({ available, granted })
  },
  PUT: async ({ request, params }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const body = await parseBody(request, z.object({ repos: z.array(z.string().max(200)).max(100) }))
    if (body instanceof Response) return body
    if (!(await agentExists(params.agentId))) return json({ error: 'unknown agent' }, { status: 404 })
    const pool = new Set(await listReachableRepos())
    const repos = body.repos.filter((r) => pool.has(r))
    await setGrantedRepos(params.agentId, repos)
    return json({ granted: repos })
  },
})
