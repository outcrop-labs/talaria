import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { getSessionUser } from '@/server/auth/session'
import { agentCaller } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, canEdit, invalidAssignee, listMembers } from '@/server/boards'
import { createTask, listBoardTasks, EFFORTS, PRIORITIES } from '@/server/tasks'
import { TICKET_COLORS } from '@/lib/task-const'
import { notifyMentions } from '@/server/mentions'
import { describeAgent } from '@/server/gateway'
import { indexTicket } from '@/server/retrieval/sources'
import { resolveTemplate } from '@/server/templates'

/** Resolves who's calling: a board-allowed agent (by key + name), an editing
 *  user, or nobody. Agents must pass the board's agent policy. */
async function taskActor(
  request: Request,
  boardId: string,
  requireEdit: boolean,
): Promise<{ actor: string; agent: boolean } | Response> {
  const caller = await agentCaller(request)
  if (caller instanceof Response) return caller
  if (caller) {
    // Pass the CALLER, not its model: board policy's elevated-assistant bypass
    // is org-wide reach, and only a caller that PROVED its identity gets it.
    if (!(await boardAllowsAgent(boardId, caller))) {
      return json({ error: `agent "${caller.model}" is not allowed on this board` }, { status: 403 })
    }
    return { actor: caller.model, agent: true }
  }
  const user = await requireUser(request)
  if (user instanceof Response) return user
  const role = await boardRole(user.id, boardId)
  if (requireEdit ? !canEdit(role) : !role) return json({ error: 'forbidden' }, { status: 403 })
  return { actor: user.email ?? user.name ?? 'user', agent: false }
}

// GET → the board's tasks (any member, or a board-allowed agent).
// POST → create a card (owner/editor, or a board-allowed agent → inbox).
export const Route = defineApi('/api/boards/$id/tasks', {
  GET: async ({ request, params }) => {
    const who = await taskActor(request, params.id, false)
    if (who instanceof Response) return who
    const includeArchived = !who.agent && new URL(request.url).searchParams.get('archived') === '1'
    return json({ tasks: await listBoardTasks(params.id, includeArchived) })
  },
  POST: async ({ request, params }) => {
    const who = await taskActor(request, params.id, true)
    if (who instanceof Response) return who
    const body = await parseBody(
      request,
      z.object({
        title: z.string().min(1).max(300),
        description: z.string().max(20_000).optional(),
        priority: z.enum(PRIORITIES).optional(),
        effort: z.enum(EFFORTS).nullish(),
        assignees: z.array(z.string().max(200)).max(20).optional(),
        dueDate: z.string().datetime().nullish(),
        startDate: z.string().datetime().nullish(),
        color: z.enum(TICKET_COLORS).nullish(),
        estimatedHours: z.number().min(0).max(999).nullish(),
        parentId: z.string().uuid().nullish(),
        tags: z.array(z.string().max(40)).max(20).optional(),
      }),
    )
    if (body instanceof Response) return body
    // Guardrail: agents create into inbox only — assignment stays a human call.
    if (who.agent && body.assignees?.length) {
      return json({ error: 'agents cannot assign tickets' }, { status: 403 })
    }
    // The same human-planning fields updateTask strips from an agent PATCH
    // (estimate, sub-task structure) are not an agent's to set at CREATION
    // either — otherwise a hand-rolled POST walks around the update gate and
    // an agent estimates its own work or re-parents the plan. Dropped rather
    // than refused, so the ticket still lands: same end state as creating it
    // and then being unable to patch these in.
    const planning = who.agent
      ? { estimatedHours: null, parentId: null }
      : { estimatedHours: body.estimatedHours ?? null, parentId: body.parentId ?? null }
    // Mixed assignees: `user:<uuid>` must be a board member; bare strings
    // are agents and must pass the board's agent policy.
    const bad = await invalidAssignee(params.id, body.assignees ?? [])
    if (bad) return json({ error: bad }, { status: 400 })
    // Templatize bare tickets: an empty description is seeded from the
    // resolved ticket template (creating agent's binding → board default),
    // so every creation surface — quick-add, agent tools — gets the format.
    let description = body.description
    if (!description?.trim()) {
      const template = await resolveTemplate('ticket', {
        agentModel: who.agent ? who.actor : null,
        boardId: params.id,
      })
      if (template?.body.trim()) description = template.body
    }
    let task
    try {
      task = await createTask({
        boardId: params.id,
        title: body.title,
        description,
        priority: body.priority,
        effort: body.effort ?? null,
        assignees: body.assignees ?? [],
        dueDate: body.dueDate ?? null,
        startDate: body.startDate ?? null,
        color: body.color ?? null,
        estimatedHours: planning.estimatedHours,
        parentId: planning.parentId,
        tags: body.tags,
        createdBy: who.actor,
      })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
    // Index into the ambient activity brain (board-scoped; retrieval on demand).
    void indexTicket(task).catch(() => {})
    // A description born with an @mention notifies board members — same
    // contract as editing one in (tasks.$id PUT). Template seeds carry no
    // mentions, so only author-written descriptions can fire this.
    if (body.description?.includes('@')) {
      const sessionUser = who.agent ? null : await getSessionUser(request)
      void notifyMentions(
        await listMembers(params.id),
        sessionUser?.id ?? '',
        who.agent ? describeAgent(who.actor).label : (sessionUser?.name ?? who.actor),
        body.description,
        task.ticketRef ?? 'a ticket',
        `/boards/${params.id}/${task.id}`,
      ).catch(() => {})
    }
    return json({ task })
  },
})
