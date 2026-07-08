import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { listRecentMessagesWithToken } from '@/server/google/gmail'
import { resolveAgentGoogle, resolveAgentPrincipal } from '@/server/google/agent-google'
import { queueAction } from '@/server/google/pending-actions'
import { googleFail } from '@/server/google/errors'

const Draft = z.object({
  to: z.string().min(3).max(500),
  subject: z.string().max(500).default(''),
  body: z.string().max(50_000).default(''),
  cc: z.string().max(500).optional(),
  bcc: z.string().max(500).optional(),
})

// Agent-facing Gmail. A personal assistant acts as its owner; a general fleet
// agent acts on the shared ORG mailbox.
// GET  → read recent mail (free)
// POST → DRAFT an email; queued for approval (the owner, or an admin for org).
export const Route = createFileRoute('/api/integrations/google/agent/gmail')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const google = await resolveAgentGoogle(name, Date.now())
        if (!google) return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
        const q = new URL(request.url).searchParams.get('q') || 'in:inbox'
        try {
          return json({ messages: await listRecentMessagesWithToken(google.token, 8, q) })
        } catch (err) {
          return googleFail(err as Error, 'Gmail')
        }
      },
      POST: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const parsed = Draft.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const principal = await resolveAgentPrincipal(name)
        const action = await queueAction({
          kind: 'gmail_send',
          summary: `Email to ${parsed.data.to}: ${parsed.data.subject || '(no subject)'}`,
          payload: parsed.data,
          agentModel: name,
          ownerUserId: principal.ownerUserId,
          isOrg: principal.isOrg,
        })
        return json({
          pending: { id: action.id, status: 'pending' },
          message: principal.isOrg ? 'Drafted — waiting for an admin to approve before it sends.' : 'Drafted — waiting for the owner to approve before it sends.',
        })
      },
    },
  },
})
