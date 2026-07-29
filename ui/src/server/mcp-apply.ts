// Applying MCP registry changes to RUNNING agents. A config re-render isn't
// enough: Hermes establishes its MCP connections at process start, so a newly
// granted server never appears inside a live container. The fix is the
// fleet's blue/green roll — the current container keeps serving until its
// replacement is healthy, in-flight replies drain, then cutover — applied to
// exactly the agents a change touches. Rolls run sequentially off a deduped
// queue: a burst of registry edits becomes one roll per department.
import { db } from './db/pg'
import { rollAgent } from './fleet-reconcile'
import { logAudit } from './audit'

const queue: string[] = []
let running = false

function enqueue(departments: string[]): void {
  for (const d of departments) if (!queue.includes(d)) queue.push(d)
  if (running) return
  running = true
  void (async () => {
    for (let d = queue.shift(); d; d = queue.shift()) {
      const r = await rollAgent(d).catch((e: Error) => ({ ok: false, error: e.message }))
      if (!r.ok) {
        void logAudit({ actor: 'talaria', action: 'mcp.roll_failed', targetType: 'department', targetId: d, after: { error: r.error } })
      }
    }
    running = false
  })()
}

/** The departments whose managed agents carry this server right now. */
export async function carriersForServer(serverId: string): Promise<string[]> {
  const sql = await db()
  const [srv] = (await sql`select all_agents as "allAgents" from mcp_servers where id = ${serverId}`) as unknown as Array<{
    allAgents: boolean
  }>
  const rows = srv?.allAgents
    ? ((await sql`select distinct department from agent_defs where managed and enabled`) as unknown as Array<{ department: string }>)
    : ((await sql`
        select distinct d.department from mcp_server_agents a
        join agent_defs d on d.model = a.agent_model and d.managed and d.enabled
        where a.server_id = ${serverId}
      `) as unknown as Array<{ department: string }>)
  return rows.map((r) => r.department)
}

/** Queue rolls for an explicit department list (e.g. captured pre-delete). */
export function enqueueRolls(departments: string[]): void {
  enqueue(departments)
}

/** Roll every managed agent that carries this server (all-agents or assigned). */
export async function rollAgentsForServer(serverId: string): Promise<void> {
  enqueue(await carriersForServer(serverId))
}

/** Roll one user's personal assistant (their connect/disconnect took effect). */
export async function rollAgentForUser(userId: string): Promise<void> {
  const sql = await db()
  const rows = (await sql`
    select department from agent_defs where owner_user_id = ${userId} and managed and enabled
  `) as unknown as Array<{ department: string }>
  enqueue(rows.map((r) => r.department))
}

/** Roll one specific agent model's department. */
export async function rollAgentForModel(model: string): Promise<void> {
  const sql = await db()
  const rows = (await sql`
    select department from agent_defs where model = ${model} and managed and enabled
  `) as unknown as Array<{ department: string }>
  enqueue(rows.map((r) => r.department))
}
