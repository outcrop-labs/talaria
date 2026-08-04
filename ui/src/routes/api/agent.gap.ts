import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { requireAgent } from '@/server/agent-auth'
import { boardAllowsAgent } from '@/server/boards'
import { reportGap } from '@/server/gaps'
import { agentTicketRefusal, getTask, logActivity } from '@/server/tasks'

const Body = z.object({
  kind: z.string().min(2).max(80),
  missing: z.string().min(5).max(300),
  needs: z.string().max(5000).optional(),
  taskId: z.string().uuid().optional(),
})

// POST — an agent reports a capability gap (the honesty loop). Deduped by
// work-shape server-side: repeats bump seen_count, never re-notify. Lands in
// the Studio's Suggested queue; the ticket (if given) gets an audit line.
export const Route = createFileRoute('/api/agent/gap')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const caller = await requireAgent(request)
        if (caller instanceof Response) return caller
        const agent = caller.model
        const body = await parseJson(request)
        const parsed = Body.safeParse(body)
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // `taskId` arrives from the agent, so it is AUTHORISED, never taken on
        // faith: the ticket gets an audit line and the gap row is bound to that
        // ticket's board. Without this an agent forges activity on any ticket
        // org-wide and binds a Studio gap to a board it cannot see. Unknown and
        // not-allowed refuse identically — a distinct 404 would be a ticket
        // enumeration oracle.
        const task = parsed.data.taskId ? await getTask(parsed.data.taskId) : null
        if (parsed.data.taskId) {
          const refuse = json(
            {
              error: 'forbidden',
              message: `taskId "${parsed.data.taskId}" is not a ticket you may write to — re-send the gap without taskId, or ask for access to its board.`,
            },
            { status: 403 },
          )
          if (!task) return refuse
          // The CALLER, not its model: board policy's elevated bypass is only
          // for an identity that was proven, never merely asserted.
          if (!(await boardAllowsAgent(task.boardId, caller))) return refuse
          // A person has taken this ticket off the table (signed off, archived,
          // or its board archived) — the gap is still worth recording, just not
          // ON that ticket. The SAME predicate `agentSafePatch` asks: this route
          // writes an activity line and never reaches `updateTask`.
          const shut = await agentTicketRefusal(task, caller, 'write')
          if (shut) {
            return json(
              { error: 'forbidden', message: `${shut}. Re-send this gap without taskId and it will still reach the Studio.` },
              { status: 403 },
            )
          }
        }
        const gap = await reportGap({
          agentModel: agent,
          kind: parsed.data.kind,
          missing: parsed.data.missing,
          needs: parsed.data.needs,
          boardId: task?.boardId ?? null,
          taskId: task?.id ?? null,
        })
        if (task) {
          await logActivity(
            task.id,
            agent,
            'gap',
            `reported a capability gap (${parsed.data.kind}${gap.seenCount > 1 ? `, seen ${gap.seenCount}×` : ''}): ${parsed.data.missing.slice(0, 200)}`,
          ).catch(() => {})
        }
        return json({
          ok: true,
          seenCount: gap.seenCount,
          note: gap.first
            ? 'Gap recorded — it will be suggested to the team in the Studio. Continue as best you can or set the ticket blocked with a comment.'
            : `Known gap (seen ${gap.seenCount}×) — already suggested to the team. Do not report it again; continue as best you can or set the ticket blocked.`,
        })
      },
    },
  },
})

async function parseJson(request: Request): Promise<unknown> {
  return request.json().catch(() => null)
}
