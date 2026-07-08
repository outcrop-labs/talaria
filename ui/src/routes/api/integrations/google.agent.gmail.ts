import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { listRecentMessages } from '@/server/google/gmail'
import { resolveAgentOwnerUser } from '@/server/google/agent-google'
import { queueAction } from '@/server/google/pending-actions'
import { googleFail } from '@/server/google/errors'

const Draft = z.object({
  to: z.string().min(3).max(500),
  subject: z.string().max(500).default(''),
  body: z.string().max(50_000).default(''),
  cc: z.string().max(500).optional(),
  bcc: z.string().max(500).optional(),
})

// Agent-facing Gmail for a PERSONAL ASSISTANT acting as its owner.
// GET  → read the owner's recent mail (free)
// POST → DRAFT an email; queued for the owner to approve, not sent now.
export const Route = createFileRoute('/api/integrations/google/agent/gmail')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const owner = await resolveAgentOwnerUser(name)
        if (!owner) return json({ error: 'not_personal', message: 'Mail access is only for a personal assistant acting for its owner.' }, { status: 403 })
        const q = new URL(request.url).searchParams.get('q') || 'in:inbox'
        try {
          return json({ messages: await listRecentMessages(owner, Date.now(), 8, q) })
        } catch (err) {
          return googleFail(err as Error, 'Gmail')
        }
      },
      POST: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const owner = await resolveAgentOwnerUser(name)
        if (!owner) return json({ error: 'not_personal', message: 'Only a personal assistant can draft mail for its owner.' }, { status: 403 })
        const parsed = Draft.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const action = await queueAction({
          kind: 'gmail_send',
          summary: `Email to ${parsed.data.to}: ${parsed.data.subject || '(no subject)'}`,
          payload: parsed.data,
          agentModel: name,
          ownerUserId: owner,
        })
        return json({ pending: { id: action.id, status: 'pending' }, message: 'Drafted — waiting for the owner to approve before it sends.' })
      },
    },
  },
})
