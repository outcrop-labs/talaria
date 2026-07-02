import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole } from '@/server/boards'
import { getTask, logActivity } from '@/server/tasks'
import { recordUsage, taskUsage } from '@/server/usage'

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
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        if (checkAgentKey(request)) {
          const name = agentName(request)
          if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
          if (!(await boardAllowsAgent(task.boardId, name))) return json({ error: 'forbidden' }, { status: 403 })
        } else {
          const user = await getSessionUser(request)
          if (!user) return json({ error: 'unauthorized' }, { status: 401 })
          if (!(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
        }
        return json(await taskUsage(params.id))
      },
      POST: async ({ request, params }) => {
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        // Usage is agent-reported (agents know what they burned); humans don't
        // post token counts by hand.
        if (!checkAgentKey(request)) return json({ error: 'agent key required' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        if (!(await boardAllowsAgent(task.boardId, name))) {
          return json({ error: `agent "${name}" is not allowed on this board` }, { status: 403 })
        }
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })

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
