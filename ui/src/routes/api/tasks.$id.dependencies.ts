import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentCaller, type AgentCaller } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, canEdit } from '@/server/boards'
import { addDependency, agentTicketRefusal, getTask, HumanApprovalRequired, removeDependency } from '@/server/tasks'

const Body = z.object({ dependsOnId: z.string().uuid() })

// POST { dependsOnId } → this ticket is blocked by another. DELETE → remove.
// Editors or board-allowed agents may add (part of triage); removal is human-only.
// The dependency target must live on the same board.
export const Route = defineApi('/api/tasks/$id/dependencies', {
  POST: async ({ request, params }) => {
    const task = await getTask(params.id)
    if (!task) return json({ error: 'not found' }, { status: 404 })
    let actor: string
    // The AGENT itself, not a boolean — the second check below needs the
    // same subject the first one used, and a bare `isAgent` flag left the
    // caller's identity to be re-narrowed at a distance.
    let agent: AgentCaller | null = null
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (caller) {
      // The CALLER, not its model: board policy's elevated-assistant bypass
      // is org-wide reach, and a legacy caller only asserted its name.
      if (!(await boardAllowsAgent(task.boardId, caller))) return json({ error: 'forbidden' }, { status: 403 })
      // The central agent-write invariant, imported rather than restated:
      // `addDependency` never reaches `updateTask` (it writes
      // task_dependencies + a task_activity line directly), so the same
      // predicate `agentSafePatch` asks is asked here — closed, archived,
      // and archived-board, all three, from one definition.
      const shut = await agentTicketRefusal(task, caller, 'write')
      if (shut) return json({ error: shut }, { status: 403 })
      actor = caller.model
      agent = caller
    } else {
      const user = await getSessionUser(request)
      if (!user || !canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
      actor = user.email ?? user.name ?? 'user'
    }
    const parsed = Body.safeParse(await request.json().catch(() => null))
    if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
    const dep = await getTask(parsed.data.dependsOnId)
    if (!dep || dep.boardId !== task.boardId) return json({ error: 'must be a ticket on this board' }, { status: 400 })
    // The edge lands on BOTH tickets (it shows in the target's "blocks"
    // list), so the rule applies to the target too.
    if (agent) {
      const depShut = await agentTicketRefusal(dep, agent, 'write')
      if (depShut) return json({ error: `${depShut}. That is the ticket you named as a blocker.` }, { status: 403 })
    }
    // `addDependency` THROWS on a cycle (X blocks Y, Y blocks X: a graph no
    // ticket in it can ever satisfy). With no catch that surfaced as an
    // unhandled 500 with the reason swallowed, so neither an agent nor a
    // person could tell a server fault from "you just asked for something
    // impossible". Same shape as tasks.$id PUT: a write that needs a person
    // is 403, a request that cannot be satisfied is 400, and both carry the
    // sentence that says why.
    try {
      await addDependency(params.id, parsed.data.dependsOnId, actor)
    } catch (e) {
      if (e instanceof HumanApprovalRequired) return json({ error: e.message }, { status: 403 })
      return json({ error: (e as Error).message }, { status: 400 })
    }
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const user = await getSessionUser(request)
    if (!user) return json({ error: 'unauthorized' }, { status: 401 })
    const task = await getTask(params.id)
    if (!task || !canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
    const parsed = Body.safeParse(await request.json().catch(() => null))
    if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
    await removeDependency(params.id, parsed.data.dependsOnId)
    return json({ ok: true })
  },
})
