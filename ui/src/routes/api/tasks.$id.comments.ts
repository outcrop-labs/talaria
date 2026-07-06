import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, listMembers } from '@/server/boards'
import { addComment, getTask, listComments } from '@/server/tasks'
import { notifyMentions } from '@/server/mentions'
import { indexTicketComment } from '@/server/retrieval/sources'

/** Board access for either a session user or a board-allowed named agent. */
async function commentAuthor(request: Request, boardId: string): Promise<string | Response> {
  if (checkAgentKey(request)) {
    const name = agentName(request)
    // Unnamed agent-key callers keep the legacy generic author.
    if (!name) return 'agent'
    if (!(await boardAllowsAgent(boardId, name))) return json({ error: 'forbidden' }, { status: 403 })
    return name
  }
  const user = await getSessionUser(request)
  if (!user || !(await boardRole(user.id, boardId))) return json({ error: 'forbidden' }, { status: 403 })
  return user.email ?? user.name ?? 'user'
}

// GET → a task's comments (board member or board-allowed agent).
// POST → add a comment (member or agent).
export const Route = createFileRoute('/api/tasks/$id/comments')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        const who = await commentAuthor(request, task.boardId)
        if (who instanceof Response) return who
        return json({ comments: await listComments(params.id) })
      },
      POST: async ({ request, params }) => {
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })
        const author = await commentAuthor(request, task.boardId)
        if (author instanceof Response) return author

        const parsed = z
          .object({ content: z.string().min(1).max(20_000), parentId: z.string().uuid().optional() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const comment = await addComment(params.id, author, parsed.data.content, parsed.data.parentId)

        // Index into the ambient activity brain (board-scoped).
        void indexTicketComment({
          id: comment.id,
          taskId: params.id,
          boardId: task.boardId,
          ticketRef: task.ticketRef,
          author,
          content: parsed.data.content,
        }).catch(() => {})

        // @mention any board member — they get an inbox notification linking to
        // the ticket. Detached; the POST returns immediately.
        const sender = await getSessionUser(request)
        void listMembers(task.boardId)
          .then((members) =>
            notifyMentions(
              members,
              sender?.id ?? '',
              sender?.name ?? author,
              parsed.data.content,
              task.ticketRef ?? 'a ticket',
              `/boards/${task.boardId}/${params.id}`,
            ),
          )
          .catch(() => {})

        return json({ comment })
      },
    },
  },
})
