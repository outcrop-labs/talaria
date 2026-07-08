import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { listUpcomingEvents } from '@/server/google/calendar'
import { resolveAgentOwnerUser } from '@/server/google/agent-google'
import { queueAction } from '@/server/google/pending-actions'
import { googleFail } from '@/server/google/errors'

const Draft = z.object({
  summary: z.string().min(1).max(500),
  description: z.string().max(8000).optional(),
  location: z.string().max(500).optional(),
  start: z.string().min(4),
  end: z.string().min(4),
  allDay: z.boolean().optional(),
  attendees: z.array(z.string().email()).max(50).optional(),
})

// Agent-facing calendar for a PERSONAL ASSISTANT acting as its owner.
// GET  → read the owner's upcoming events (free)
// POST → DRAFT an event; it's queued for the owner to approve, not created now.
export const Route = createFileRoute('/api/integrations/google/agent/calendar')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const owner = await resolveAgentOwnerUser(name)
        if (!owner) return json({ error: 'not_personal', message: 'Calendar access is only for a personal assistant acting for its owner.' }, { status: 403 })
        try {
          return json({ events: await listUpcomingEvents(owner, Date.now()) })
        } catch (err) {
          return googleFail(err as Error, 'Calendar')
        }
      },
      POST: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const owner = await resolveAgentOwnerUser(name)
        if (!owner) return json({ error: 'not_personal', message: 'Only a personal assistant can draft events for its owner.' }, { status: 403 })
        const parsed = Draft.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const action = await queueAction({
          kind: 'calendar_create',
          summary: `Event: ${parsed.data.summary} (${parsed.data.start})`,
          payload: parsed.data,
          agentModel: name,
          ownerUserId: owner,
        })
        return json({ pending: { id: action.id, status: 'pending' }, message: 'Drafted — waiting for the owner to approve.' })
      },
    },
  },
})
