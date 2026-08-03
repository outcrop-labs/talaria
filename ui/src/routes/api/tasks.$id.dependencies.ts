import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentCaller } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, canEdit } from '@/server/boards'
import { addDependency, getTask, removeDependency } from '@/server/tasks'
import { statusMeta } from '@/server/statuses'

const Body = z.object({ dependsOnId: z.string().uuid() })

/** Off-board terminal keys — mirrors OFF_BOARD_STATUSES in server/tasks.ts. */
const OFF_BOARD_STATUSES = ['failed', 'cancelled']

/** The closed-ticket clause of the central invariant (`agentSafePatch`, in
 *  server/tasks.ts), applied here because `addDependency` never reaches
 *  `updateTask`: it writes task_dependencies + a task_activity line directly.
 *  FOLLOW-UP: server/tasks.ts should export this as one predicate so the four
 *  agent-reachable side doors can't drift from the invariant — another agent
 *  owns that file this round, so it is mirrored rather than shared. */
async function closedToAgents(task: { boardId: string; status: string }): Promise<boolean> {
  const meta = await statusMeta(task.boardId)
  return meta.doneKeys.includes(task.status) || OFF_BOARD_STATUSES.includes(task.status)
}

// POST { dependsOnId } → this ticket is blocked by another. DELETE → remove.
// Editors or board-allowed agents may add (part of triage); removal is human-only.
// The dependency target must live on the same board.
export const Route = createFileRoute('/api/tasks/$id/dependencies')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        let actor: string
        let isAgent = false
        const caller = await agentCaller(request)
        if (caller instanceof Response) return caller
        if (caller) {
          // The CALLER, not its model: board policy's elevated-assistant bypass
          // is org-wide reach, and a legacy caller only asserted its name.
          if (!(await boardAllowsAgent(task.boardId, caller))) return json({ error: 'forbidden' }, { status: 403 })
          // Sign-off is sticky and covers the RECORD: a dependency edge is a
          // write to a ticket a person closed, so an agent doesn't get one.
          if (await closedToAgents(task)) {
            return json({ error: 'agents cannot change a closed ticket' }, { status: 403 })
          }
          actor = caller.model
          isAgent = true
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
        // list), so the closed rule applies to the target too.
        if (isAgent && (await closedToAgents(dep))) {
          return json({ error: 'agents cannot change a closed ticket — the ticket you named as a blocker is signed off' }, { status: 403 })
        }
        await addDependency(params.id, parsed.data.dependsOnId, actor)
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
    },
  },
})
