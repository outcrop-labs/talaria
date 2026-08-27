import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody } from '@/server/api-guard'
import { requireAgent } from '@/server/agent-auth'
import { boardAllowsAgent } from '@/server/boards'
import { rememberTicketRefusal, reportGap } from '@/server/gaps'
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
export const Route = defineApi('/api/agent/gap', {
  POST: async ({ request }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    const agent = caller.model
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    // `taskId` arrives from the agent, so it is AUTHORISED, never taken on
    // faith: the ticket gets an audit line and the gap row is bound to that
    // ticket's board. Without this an agent forges activity on any ticket
    // org-wide and binds a Studio gap to a board it cannot see. Unknown and
    // not-allowed refuse identically — a distinct 404 would be a ticket
    // enumeration oracle.
    // NEVER tell a refused agent to drop `taskId` and retry. `boardId` is
    // what narrows the gap's free text to the admins who can see that board
    // (`{ by: 'admin', onBoard }`), and that text routinely names the
    // ticket, the customer or the file. "Re-send without taskId" was
    // therefore an instruction to widen its own disclosure — a refusal that
    // reads as a workaround.
    //
    // AND SAYING NOTHING IS NOT ENOUGH. The sentence was removed from this
    // body and from the MCP tool description an agent reads before it ever
    // calls — and the retry still worked, because the refusal left no trace
    // and the next unbounded report defaulted to every admin. So each
    // refusal below is REMEMBERED against the caller, and `reportGap` reads
    // that memo when a report arrives with no ticket on it. See "WHAT AN
    // AGENT'S OWN WORDS MAY BE DISCLOSED AS" in server/gaps.ts for the
    // authority each case resolves to; an agent that names no ticket and was
    // refused nothing is making a genuinely org-wide claim ("I cannot send
    // email at all") and is unaffected.
    const task = body.taskId ? await getTask(body.taskId) : null
    if (body.taskId) {
      // Unknown and not-allowed refuse identically — a distinct 404 would be
      // a ticket enumeration oracle.
      const refuse = json(
        {
          error: 'forbidden',
          message: `taskId "${body.taskId}" is not a ticket you may write to. Ask for access to its board, then report the gap against the ticket.`,
        },
        { status: 403 },
      )
      // Remembered with NO board in both of these: the agent named a ticket
      // that does not exist, or one on a board it may not work. Binding its
      // next report to a board it cannot see would let it choose which
      // board's admins read its text, so it gets no board and its retry
      // reaches every admin as a fact and none of them as words.
      if (!task) {
        await rememberTicketRefusal(agent, null)
        return refuse
      }
      // The CALLER, not its model: board policy's elevated bypass is only
      // for an identity that was proven, never merely asserted.
      if (!(await boardAllowsAgent(task.boardId, caller))) {
        await rememberTicketRefusal(agent, null)
        return refuse
      }
      // A person has taken this ticket off the table (signed off, archived,
      // or its board archived). The SAME predicate `agentSafePatch` asks:
      // this route writes an activity line and never reaches `updateTask`.
      // The agent IS allowed on this board, so this is the one refusal whose
      // board we can safely bind a retry to.
      const shut = await agentTicketRefusal(task, caller, 'write')
      if (shut) {
        await rememberTicketRefusal(agent, task.boardId)
        return json({ error: 'forbidden', message: shut }, { status: 403 })
      }
    }
    const gap = await reportGap({
      agentModel: agent,
      kind: body.kind,
      missing: body.missing,
      needs: body.needs,
      boardId: task?.boardId ?? null,
      taskId: task?.id ?? null,
    })
    if (task) {
      await logActivity(
        task.id,
        agent,
        'gap',
        `reported a capability gap (${body.kind}${gap.seenCount > 1 ? `, seen ${gap.seenCount}×` : ''}): ${body.missing.slice(0, 200)}`,
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
})
