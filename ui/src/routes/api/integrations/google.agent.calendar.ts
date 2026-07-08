import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { listUpcomingEventsWithToken } from '@/server/google/calendar'
import { resolveAgentGoogle, resolveAgentPrincipal } from '@/server/google/agent-google'
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

// Agent-facing calendar. A personal assistant acts as its owner; a general fleet
// agent acts on the shared ORG calendar.
// GET  → read upcoming events (free)
// POST → DRAFT an event; queued for approval (the owner, or an admin for org).
export const Route = createFileRoute('/api/integrations/google/agent/calendar')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!checkAgentKey(request)) return json({ error: 'unauthorized' }, { status: 401 })
        const name = agentName(request)
        if (!name) return json({ error: 'x-agent-name required' }, { status: 400 })
        const google = await resolveAgentGoogle(name, Date.now())
        if (!google) return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
        try {
          return json({ events: await listUpcomingEventsWithToken(google.token, Date.now()) })
        } catch (err) {
          return googleFail(err as Error, 'Calendar')
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
          kind: 'calendar_create',
          summary: `Event: ${parsed.data.summary} (${parsed.data.start})`,
          payload: parsed.data,
          agentModel: name,
          ownerUserId: principal.ownerUserId,
          isOrg: principal.isOrg,
        })
        return json({
          pending: { id: action.id, status: 'pending' },
          message: principal.isOrg ? 'Drafted — waiting for an admin to approve.' : 'Drafted — waiting for the owner to approve.',
        })
      },
    },
  },
})
