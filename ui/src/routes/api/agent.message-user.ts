import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { agentMessageUser } from '@/server/outreach'

const Body = z.object({
  /** Teammate's email (preferred) or exact display name. */
  to: z.string().min(1).max(200),
  message: z.string().min(1).max(4000),
})

// POST (agent key) → an agent starts or continues a direct conversation with
// a human teammate. The message lands as a normal turn in their chat with
// this agent plus an inbox notification. Personal assistants reach only
// their owner; every agent↔user pair is rate-capped per day.
export const Route = createFileRoute('/api/agent/message-user')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const agent = agentName(request)
        if (!agent) return json({ error: 'x-agent-name required' }, { status: 400 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const result = await agentMessageUser(agent, parsed.data.to, parsed.data.message)
        if (!result.ok) return json({ error: result.error }, { status: 422 })
        return json({ ok: true, conversationId: result.conversationId })
      },
    },
  },
})
