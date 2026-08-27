import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { getSessionUser } from '@/server/auth/session'
import { parseBody, requireUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, listMembers } from '@/server/boards'
import { addComment, agentTicketRefusal, getTask, listComments, type AgentWriteTarget } from '@/server/tasks'
import { notifyMentions } from '@/server/mentions'
import { indexTicketComment } from '@/server/retrieval/sources'

/** Who may READ this ticket's comments: a board member, or an agent the board's
 *  policy allows. Reading changes nothing, so it is the BOARD question only —
 *  an agent that can see the board can read the thread on a ticket that is
 *  closed or archived. */
async function commentReader(request: Request, boardId: string): Promise<string | Response> {
  const caller = await agentCaller(request)
  if (caller instanceof Response) return caller
  if (caller) {
    // The CALLER, not its model: the elevated-assistant bypass inside board
    // policy is org-wide reach, and a legacy caller only asserted its name.
    if (!(await boardAllowsAgent(boardId, caller))) return json({ error: 'forbidden' }, { status: 403 })
    return caller.model
  }
  const gate = await requireUser(request)
  if (gate instanceof Response) return gate
  const user = gate
  if (!(await boardRole(user.id, boardId))) return json({ error: 'forbidden' }, { status: 403 })
  return user.email ?? user.name ?? 'user'
}

/** Who may POST a comment. Posting is an ACT on the ticket, so an agent goes
 *  through the one agent-authority predicate with intent 'comment' — board
 *  policy plus archival, and deliberately NOT the closed-status clause, because
 *  commenting is the agent's channel on work it can no longer edit.
 *
 *  This gate used to be `boardAllowsAgent` alone, which meant an archived BOARD
 *  refused agent comments (board policy reads `archived_at`) while an archived
 *  TICKET accepted them — one act of a person meaning two different things, one
 *  level apart. `agentTicketRefusal` is where that is now decided once; see the
 *  reasoning on `AgentIntent`. */
async function commentAuthor(request: Request, task: AgentWriteTarget): Promise<string | Response> {
  const caller = await agentCaller(request)
  if (caller instanceof Response) return caller
  if (caller) {
    const shut = await agentTicketRefusal(task, caller, 'comment')
    if (shut) return json({ error: shut }, { status: 403 })
    return caller.model
  }
  const gate = await requireUser(request)
  if (gate instanceof Response) return gate
  const user = gate
  if (!(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
  return user.email ?? user.name ?? 'user'
}

// GET → a task's comments (board member or board-allowed agent).
// POST → add a comment (member or agent).
export const Route = defineApi('/api/tasks/$id/comments', {
  GET: async ({ request, params }) => {
    const task = await getTask(params.id)
    if (!task) return json({ error: 'not found' }, { status: 404 })
    const who = await commentReader(request, task.boardId)
    if (who instanceof Response) return who
    return json({ comments: await listComments(params.id) })
  },
  POST: async ({ request, params }) => {
    const task = await getTask(params.id)
    if (!task) return json({ error: 'not found' }, { status: 404 })
    const author = await commentAuthor(request, task)
    if (author instanceof Response) return author

    const parsed = await parseBody(
      request,
      z.object({ content: z.string().min(1).max(20_000), parentId: Uuid.optional() }),
    )
    if (parsed instanceof Response) return parsed
    const comment = await addComment(params.id, author, parsed.content, parsed.parentId)

    // Index into the ambient activity brain (board-scoped).
    void indexTicketComment({
      id: comment.id,
      taskId: params.id,
      boardId: task.boardId,
      ticketRef: task.ticketRef,
      author,
      // `comment.content`, NOT `parsed.data.content`: `addComment` runs an
      // agent's comment through the agent-writes door and returns the REDACTED
      // body, so the raw one reaching the index put a credential into the
      // retrieval brain — which is read back into model contexts, the exact
      // re-entry guardrails.ts's cardinal invariant forbids. The persisted
      // comment was already clean; only these copies were not.
      content: comment.content,
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
          // Redacted, for the same reason as the index above: this text lands
          // in a person's inbox, and it must not be the one copy of an agent's
          // comment that still carries the credential the ticket no longer does.
          comment.content,
          task.ticketRef ?? 'a ticket',
          `/boards/${task.boardId}/${params.id}`,
        ),
      )
      .catch(() => {})

    return json({ comment })
  },
})
