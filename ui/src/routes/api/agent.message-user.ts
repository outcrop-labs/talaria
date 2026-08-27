import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody } from '@/server/api-guard'
import { requireAgent } from '@/server/agent-auth'
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
export const Route = defineApi('/api/agent/message-user', {
  POST: async ({ request }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    const agent = caller.model
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const result = await agentMessageUser(agent, body.to, body.message)
    if (!result.ok) return json({ error: result.error }, { status: 400 })
    return json({ ok: true, conversationId: result.conversationId })
  },
})
