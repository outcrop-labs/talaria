import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requirePerm } from '@/server/api-guard'
import { db } from '@/server/db/pg'

// AGENT HIRES — what the roster shows while an agent-hire run works: every
// live hire, plus the recently-finished ones long enough for the surface to
// see the transition (and a failure's sentence) before the row goes away.
//
// The def module import is for REGISTRATION only: a process that lists hires
// can also be the process a reclaim sweep asks to resume one, and a kind this
// route never registered would be a run nothing can drive.
import '@/server/runs/defs/agent-hire'

export interface AgentHireView {
  id: string
  name: string
  slug: string
  department: string
  start: boolean
  state: 'queued' | 'running' | 'done' | 'error' | 'cancelled'
  phase: string
  error: string | null
  createdAt: string
}

export const Route = defineApi('/api/fleet/hires', {
  GET: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const sql = await db()
    const rows = (await sql`
      select id, input, state, phase, error, created_at as "createdAt"
      from runs
      where kind = 'agent-hire'
        and (state in ('queued', 'running')
             or (state in ('done', 'error') and updated_at > now() - interval '10 minutes'))
      order by created_at desc
      limit 12
    `) as unknown as Array<{
      id: string
      input: { displayName?: string; slug?: string; department?: string; start?: boolean }
      state: AgentHireView['state']
      phase: string
      error: string | null
      createdAt: string
    }>
    const hires: AgentHireView[] = rows.map((r) => ({
      id: r.id,
      name: r.input.displayName ?? r.input.slug ?? 'the new agent',
      slug: r.input.slug ?? '',
      department: r.input.department ?? '',
      start: r.input.start ?? true,
      state: r.state,
      phase: r.phase,
      error: r.error,
      createdAt: r.createdAt,
    }))
    return json({ hires })
  },
})
