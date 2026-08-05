import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { requireAgent } from '@/server/agent-auth'
import { audienceFor } from '@/server/approvals'
import { boardAllowsAgent, createBoard, listAllBoards, setBoardAgentConfig } from '@/server/boards'
import { agentTicketRefusal, createTask, getTask } from '@/server/tasks'
import { addNotification } from '@/server/notifications'
import { agentTextAuthority, rememberTicketRefusal } from '@/server/gaps'
import { describeAgent } from '@/server/gateway'
import { db } from '@/server/db/pg'

const Body = z.object({
  summary: z.string().min(5).max(300),
  details: z.string().max(20_000).optional(),
  context: z.string().max(500).optional().describe('what the agent was trying to do'),
  taskId: z.string().uuid().optional().describe('the ticket the agent was working when it broke'),
})

const HELPDESK = 'Helpdesk'

/** The Helpdesk board — find-or-create, owned by the earliest admin, open to
 *  every agent so any of them can file. */
async function helpdeskBoard(): Promise<{ id: string } | null> {
  const existing = (await listAllBoards()).find((b) => b.name.toLowerCase() === HELPDESK.toLowerCase())
  if (existing) return { id: existing.id }
  const sql = await db()
  const [admin] = (await sql`
    select id from users order by (role = 'admin') desc, created_at asc limit 1
  `) as unknown as Array<{ id: string }>
  if (!admin) return null
  const board = await createBoard(admin.id, HELPDESK, null)
  await setBoardAgentConfig(board.id, true, [])
  return { id: board.id }
}

// POST (agent key) → an agent hit something broken it shouldn't explain to a
// normal person. Talaria elevates it: the admins who may hear it get an alert
// notification, a Helpdesk ticket carries the technical details (board
// find-or-created), and the agent gets plain-language confirmation to relay.
//
// THE ROUTE LEARNS WHICH TICKET THE AGENT WAS ON. It was the last member of the
// class this milestone is about: `select id from users where role = 'admin'`,
// then an `addNotification` per admin carrying `${label} hit a problem:
// ${summary}` as the title and `context` as the body — both AGENT FREE TEXT,
// typed while working one board's ticket and routinely quoting it — plus a deep
// link to a Helpdesk ticket holding up to 20 000 characters of `details`. Every
// admin in the workspace got all of that, board membership or not. The census in
// scripts/check-invariants.mjs carried it as debt with the reason: "the route
// never learns which ticket the agent was on".
//
// So it takes one. `taskId` arrives from the agent and is AUTHORISED exactly the
// way agent.gap.ts authorises its own — unknown and not-allowed refuse
// identically (a distinct 404 is a ticket enumeration oracle), and a ticket a
// person has taken off the table refuses too. Then the authority is
// `agentTextAuthority`, the same answer the capability gap gets, and the
// audience is `audienceFor` like everything else.
//
// WHAT EACH HALF IS TOLD. `content` — the admins who can see that board, or
// every admin when the problem is genuinely org-wide — gets the summary, the
// context and the link. `fact` gets the title without the summary, no body text
// the agent wrote, and NO LINK, because the link is to a ticket carrying the
// details. They are told an agent is blocked, which is the thing they can act
// on: grant the access, fix the credential, add themselves to the board.
//
// KNOWN RESIDUAL, stated rather than hidden: the Helpdesk ticket itself is not
// scoped by the originating board, so an admin who is a member of the Helpdesk
// board can read the details there. That is a PULL surface someone chose to
// open — the same residual server/gaps.ts records for the Studio's Suggested
// queue — not a push into an inbox and a mailbox. (In practice Helpdesk is
// created with a single member, the earliest admin, which is narrower than the
// notification ever was.)
export const Route = defineApi('/api/agent/problem', {
  POST: async ({ request }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    const agent = caller.model
    const parsed = Body.safeParse(await request.json().catch(() => null))
    if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
    const label = describeAgent(agent).label

    // NEVER tell a refused agent to drop `taskId` and retry, and do not
    // leave the retry working in silence either: each refusal is remembered
    // against the caller and `agentTextAuthority` reads that memo below.
    const task = parsed.data.taskId ? await getTask(parsed.data.taskId) : null
    if (parsed.data.taskId) {
      const refuse = json(
        {
          error: 'forbidden',
          message: `taskId "${parsed.data.taskId}" is not a ticket you may write to. Ask for access to its board, then report the problem against the ticket.`,
        },
        { status: 403 },
      )
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
      const shut = await agentTicketRefusal(task, caller, 'write')
      if (shut) {
        await rememberTicketRefusal(agent, task.boardId)
        return json({ error: 'forbidden', message: shut }, { status: 403 })
      }
    }

    // Ticket first (it carries the technical substance).
    let ticketNote = 'a Helpdesk ticket could not be filed'
    let href = '/observability?tab=alerts'
    const board = await helpdeskBoard().catch(() => null)
    if (board) {
      const filed = await createTask({
        boardId: board.id,
        title: `[${label}] ${parsed.data.summary}`,
        description:
          `**Reported by agent:** ${label} (${agent})\n\n` +
          (parsed.data.context ? `**While:** ${parsed.data.context}\n\n` : '') +
          (task ? `**On ticket:** [${task.title}](/boards/${task.boardId}/${task.id})\n\n` : '') +
          `**Technical details:**\n\n${parsed.data.details ?? '(none provided)'}`,
        priority: 'high',
        createdBy: agent,
      })
      ticketNote = `Helpdesk ticket filed`
      href = `/boards/${board.id}/${filed.id}`
    }

    // ONE announcement path, and it asks the resolver.
    const authority = await agentTextAuthority(agent, task?.boardId ?? null)
    const who = await audienceFor(authority)
    // WHY the fact recipient is not being told, and it is two different
    // reasons. `{ by: 'admin', onBoard }` withheld the words because the
    // reader is not on the board they quote. `{ by: 'nobody' }` withheld
    // them because the agent was refused a ticket and retried without one,
    // so this is not an org-wide report and there is no board to name — and
    // in THAT case the reader may well be a member of the board the text is
    // actually about. Printing the first sentence for both told an on-board
    // admin they were not a member of work they are a member of, which is
    // the announcement lying about its own reason. server/gaps.ts splits the
    // same two cases with the same words; the wording is a promise about
    // what happened, so both subjects have to keep it.
    const placed = authority.by === 'admin' && !!authority.onBoard
    for (const userId of who.content) {
      await addNotification(userId, {
        kind: 'agent-problem',
        title: `${label} hit a problem: ${parsed.data.summary}`,
        body: parsed.data.context ?? '',
        href,
      }).catch((e: unknown) => console.error(`[agent-problem] could not notify ${userId}:`, e))
    }
    for (const userId of who.fact) {
      await addNotification(userId, {
        kind: 'agent-problem',
        title: `${label} reported a problem it is blocked on`,
        body:
          (placed
            ? 'It was raised against work you are not a member of, so what the agent wrote is not repeated here.'
            : 'It was raised against a ticket the agent was refused, so it is not an org-wide report and what the agent wrote is not repeated here.') +
          `\n\n${ticketNote === 'Helpdesk ticket filed' ? 'The technical details are on the Helpdesk board.' : 'No Helpdesk ticket could be filed for it.'}`,
      }).catch((e: unknown) => console.error(`[agent-problem] could not notify ${userId} that a problem exists:`, e))
    }

    return json({
      ok: true,
      ticket: href !== '/observability?tab=alerts' ? href : null,
      // The exact reassurance the agent should relay, so the wording stays
      // consistent and plain.
      relay: `The workspace admin has been notified and a ${ticketNote.toLowerCase().includes('filed') ? 'helpdesk ticket was filed' : 'report was sent'} — no action needed on your side.`,
    })
  },
})
