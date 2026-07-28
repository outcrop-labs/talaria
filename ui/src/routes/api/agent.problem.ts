import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { createBoard, listAllBoards, setBoardAgentConfig } from '@/server/boards'
import { createTask } from '@/server/tasks'
import { addNotification } from '@/server/notifications'
import { describeAgent } from '@/server/gateway'
import { db } from '@/server/db/pg'

const Body = z.object({
  summary: z.string().min(5).max(300),
  details: z.string().max(20_000).optional(),
  context: z.string().max(500).optional().describe('what the agent was trying to do'),
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
// normal person. Talaria elevates it: every admin gets an alert notification,
// a Helpdesk ticket carries the technical details (board find-or-created),
// and the agent gets plain-language confirmation to relay.
export const Route = createFileRoute('/api/agent/problem')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const agent = agentName(request)
        if (!agent) return json({ error: 'x-agent-name required' }, { status: 400 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const label = describeAgent(agent).label
        const sql = await db()

        // Ticket first (it carries the technical substance).
        let ticketNote = 'a Helpdesk ticket could not be filed'
        let href = '/observability?tab=alerts'
        const board = await helpdeskBoard().catch(() => null)
        if (board) {
          const task = await createTask({
            boardId: board.id,
            title: `[${label}] ${parsed.data.summary}`,
            description:
              `**Reported by agent:** ${label} (${agent})\n\n` +
              (parsed.data.context ? `**While:** ${parsed.data.context}\n\n` : '') +
              `**Technical details:**\n\n${parsed.data.details ?? '(none provided)'}`,
            priority: 'high',
            createdBy: agent,
          })
          ticketNote = `Helpdesk ticket filed`
          href = `/boards/${board.id}/${task.id}`
        }

        // Every admin gets the alert notification.
        const admins = (await sql`select id from users where role = 'admin'`) as unknown as Array<{ id: string }>
        for (const a of admins) {
          await addNotification(a.id, {
            kind: 'agent-problem',
            title: `${label} hit a problem: ${parsed.data.summary}`,
            body: parsed.data.context ?? '',
            href,
          }).catch(() => {})
        }

        return json({
          ok: true,
          ticket: href !== '/observability?tab=alerts' ? href : null,
          // The exact reassurance the agent should relay, so the wording stays
          // consistent and plain.
          relay: `The workspace admin has been notified and a ${ticketNote.toLowerCase().includes('filed') ? 'helpdesk ticket was filed' : 'report was sent'} — no action needed on your side.`,
        })
      },
    },
  },
})
