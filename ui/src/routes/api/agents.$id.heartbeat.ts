import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { fleetCaller } from '@/server/agent-auth'
import { heartbeatAgent } from '@/server/agents-registry'
import { db } from '@/server/db/pg'
import { assignedWork } from '@/server/tasks'

// GET /api/agents/:id/heartbeat — refresh last_seen and return the agent's
// assigned work (tasks assigned to it, across boards). MC-compatible.
export const Route = defineApi('/api/agents/$id/heartbeat', {
  GET: async ({ request, params }) => {
    // Fleet-plane: the subject is the :id in the URL. Work items carry
    // ticket titles and descriptions, so a caller we CAN name must be that
    // agent — otherwise agent A enumerates agent B's assignments. A legacy
    // container that sends no x-agent-name is unnameable and still allowed;
    // that ends when the shared key does.
    const caller = await fleetCaller(request)
    if (caller instanceof Response) return caller
    if (!caller) return json({ error: 'unauthorized' }, { status: 401 })

    // AUTHORIZE, THEN WRITE. `heartbeatAgent()` is a write — it stamps
    // last_seen and lifts offline → idle — so resolving the subject through
    // it meant agent A stamped agent B's liveness and was refused only
    // afterwards: the 403 was honest but the side effect had already landed,
    // which is enough to forge another agent's presence (it reads live in
    // the fleet UI and to anything keyed off FRESH_MS). Name the subject
    // with a read first, decide, and only then heartbeat.
    const sql = await db()
    const rows = (await sql`select name from fleet_agents where id = ${params.id}`) as unknown as Array<{
      name: string
    }>
    const name = rows[0]?.name
    if (!name) return json({ error: 'unknown agent' }, { status: 404 })
    if (caller.model && caller.model !== name) {
      return json({ error: `this credential belongs to "${caller.model}", not "${name}"` }, { status: 403 })
    }

    await heartbeatAgent(params.id)
    return json({ work_items: await assignedWork(name) })
  },
})
