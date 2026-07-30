import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, canEdit, invalidAssignee, listMembers } from '@/server/boards'
import { createTask, listBoardTasks, EFFORTS, PRIORITIES } from '@/server/tasks'
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
  if (checkAgentKey(request)) {
    const agent = agentName(request)
    if (!agent) return json({ error: 'x-agent-name required' }, { status: 400 })
    if (!(await boardAllowsAgent(boardId, agent))) {
      return json({ error: `agent "${agent}" is not allowed on this board` }, { status: 403 })
    }
    return { actor: agent, agent: true }
  }
  const user = await getSessionUser(request)
  if (!user) return json({ error: 'unauthorized' }, { status: 401 })
  const role = await boardRole(user.id, boardId)
  if (requireEdit ? !canEdit(role) : !role) return json({ error: 'forbidden' }, { status: 403 })
  return { actor: user.email ?? user.name ?? 'user', agent: false }
}

// GET → the board's tasks (any member, or a board-allowed agent).
// POST → create a card (owner/editor, or a board-allowed agent → inbox).
export const Route = createFileRoute('/api/boards/$id/tasks')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const who = await taskActor(request, params.id, false)
        if (who instanceof Response) return who
        const includeArchived = !who.agent && new URL(request.url).searchParams.get('archived') === '1'
        return json({ tasks: await listBoardTasks(params.id, includeArchived) })
      },
      POST: async ({ request, params }) => {
        const who = await taskActor(request, params.id, true)
        if (who instanceof Response) return who
        const parsed = z
          .object({
            title: z.string().min(1).max(300),
            description: z.string().max(20_000).optional(),
            priority: z.enum(PRIORITIES).optional(),
            effort: z.enum(EFFORTS).nullish(),
            assignees: z.array(z.string().max(200)).max(20).optional(),
            dueDate: z.string().datetime().nullish(),
            estimatedHours: z.number().min(0).max(999).nullish(),
            parentId: z.string().uuid().nullish(),
            tags: z.array(z.string().max(40)).max(20).optional(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // Guardrail: agents create into inbox only — assignment stays a human call.
        if (who.agent && parsed.data.assignees?.length) {
          return json({ error: 'agents cannot assign tickets' }, { status: 403 })
        }
        // Mixed assignees: `user:<uuid>` must be a board member; bare strings
        // are agents and must pass the board's agent policy.
        const bad = await invalidAssignee(params.id, parsed.data.assignees ?? [])
        if (bad) return json({ error: bad }, { status: 400 })
        // Templatize bare tickets: an empty description is seeded from the
        // resolved ticket template (creating agent's binding → board default),
        // so every creation surface — quick-add, agent tools — gets the format.
        let description = parsed.data.description
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
            title: parsed.data.title,
            description,
            priority: parsed.data.priority,
            effort: parsed.data.effort ?? null,
            assignees: parsed.data.assignees ?? [],
            dueDate: parsed.data.dueDate ?? null,
            estimatedHours: parsed.data.estimatedHours ?? null,
            parentId: parsed.data.parentId ?? null,
            tags: parsed.data.tags,
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
        if (parsed.data.description?.includes('@')) {
          const sessionUser = who.agent ? null : await getSessionUser(request)
          void notifyMentions(
            await listMembers(params.id),
            sessionUser?.id ?? '',
            who.agent ? describeAgent(who.actor).label : (sessionUser?.name ?? who.actor),
            parsed.data.description,
            task.ticketRef ?? 'a ticket',
            `/boards/${params.id}/${task.id}`,
          ).catch(() => {})
        }
        return json({ task })
      },
    },
  },
})
