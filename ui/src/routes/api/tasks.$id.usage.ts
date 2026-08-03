import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentCaller, requireAgent } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole } from '@/server/boards'
import { getTask, logActivity } from '@/server/tasks'
import { statusMeta } from '@/server/statuses'
import { routedModelFor } from '@/server/fleet-agents'
import { recordUsage, taskUsage } from '@/server/usage'

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

/** Off-board terminal keys — mirrors OFF_BOARD_STATUSES in server/tasks.ts. */
const OFF_BOARD_STATUSES = ['failed', 'cancelled']

/** The closed-ticket clause of the central invariant (`agentSafePatch`, in
 *  server/tasks.ts), applied here because this route never reaches
 *  `updateTask`: it writes a spend row keyed to the ticket AND an activity
 *  line onto it. FOLLOW-UP: server/tasks.ts should export this as one
 *  predicate — another agent owns that file this round. */
async function closedToAgents(task: { boardId: string; status: string }): Promise<boolean> {
  const meta = await statusMeta(task.boardId)
  return meta.doneKeys.includes(task.status) || OFF_BOARD_STATUSES.includes(task.status)
}

const Body = z.object({
  promptTokens: z.number().int().min(0).max(100_000_000),
  completionTokens: z.number().int().min(0).max(100_000_000),
  /** Model tier the work ran on (alias name); defaults to the agent's main. */
  tier: z.string().max(60).nullish(),
  estimated: z.boolean().optional(),
})

// Per-ticket token spend. POST (agents, via MCP log_usage): report tokens
// burned working this ticket — attributed to the agent's serving endpoint and
// priced like every other ledger row. GET: the rollup shown on the ticket.
export const Route = createFileRoute('/api/tasks/$id/usage')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        // Agents pass taskId verbatim — a non-UUID must 404, not 500 on the cast.
        if (!UUID_RE.test(params.id)) return json({ error: 'not found' }, { status: 404 })
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        const caller = await agentCaller(request)
        if (caller instanceof Response) return caller
        if (caller) {
          // The CALLER, not its model — the elevated bypass inside board policy
          // is org-wide reach, and a legacy caller only asserted its name.
          if (!(await boardAllowsAgent(task.boardId, caller))) return json({ error: 'forbidden' }, { status: 403 })
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
        }
        return json(await taskUsage(params.id))
      },
      POST: async ({ request, params }) => {
        if (!UUID_RE.test(params.id)) return json({ error: 'not found' }, { status: 404 })
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        // Usage is agent-reported (agents know what they burned); humans don't
        // post token counts by hand.
        const poster = await requireAgent(request)
        if (poster instanceof Response) return poster
        const name = poster.model
        if (!(await boardAllowsAgent(task.boardId, poster))) {
          return json({ error: `agent "${name}" is not allowed on this board` }, { status: 403 })
        }
        // Closed work is what the org signed off on: no more cost lands on it,
        // and no more activity lines. Same rule as every other agent write.
        if (await closedToAgents(task)) {
          return json(
            { error: 'agents cannot change a closed ticket — this ticket is signed off, so no further spend attaches to it' },
            { status: 403 },
          )
        }
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // A tier must be one of the agent's real alias names — reject typos and
        // routed-model ids loudly instead of silently recording an
        // unattributable (and therefore unpriceable) row.
        if (parsed.data.tier && !(await routedModelFor(name, parsed.data.tier).catch(() => null))) {
          return json({ error: `unknown tier "${parsed.data.tier}" for ${name} — use an alias name or omit` }, { status: 400 })
        }

        await recordUsage({
          agentModel: name,
          source: 'ticket',
          refId: params.id,
          taskId: params.id,
          tier: parsed.data.tier ?? null,
          promptTokens: parsed.data.promptTokens,
          completionTokens: parsed.data.completionTokens,
          estimated: parsed.data.estimated ?? false,
        })
        const total = parsed.data.promptTokens + parsed.data.completionTokens
        await logActivity(params.id, name, 'usage', `logged ${total.toLocaleString('en-US')} tokens`)
        return json({ ok: true })
      },
    },
  },
})
