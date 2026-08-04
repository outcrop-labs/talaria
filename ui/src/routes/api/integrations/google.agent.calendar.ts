import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { refuseLegacy, requireAgent } from '@/server/agent-auth'
import { listUpcomingEventsWithToken } from '@/server/google/calendar'
import { resolveAgentGoogle, resolveAgentPrincipal } from '@/server/google/agent-google'
import { queueAction } from '@/server/google/pending-actions'
import { getOrgTargets } from '@/server/google/org-connection'
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
        const caller = await requireAgent(request)
        if (caller instanceof Response) return caller
        // Acting as a HUMAN — the owner's calendar (or the shared org one). A
        // legacy shared-key caller only ASSERTS which agent it is, so it never
        // reaches a token; the refusal names the container to roll.
        const denied = refuseLegacy(caller, 'Calendar access')
        if (denied) return denied
        const name = caller.model
        const google = await resolveAgentGoogle(name, Date.now())
        if (!google) return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
        const calendarId = google.principal === 'org' ? (await getOrgTargets()).calendarId : null
        try {
          return json({ events: await listUpcomingEventsWithToken(google.token, Date.now(), 10, calendarId) })
        } catch (err) {
          return googleFail(err as Error, 'Calendar')
        }
      },
      POST: async ({ request }) => {
        const caller = await requireAgent(request)
        if (caller instanceof Response) return caller
        // Acting as a HUMAN — the owner's calendar (or the shared org one). A
        // legacy shared-key caller only ASSERTS which agent it is, so it never
        // reaches a token; the refusal names the container to roll.
        const denied = refuseLegacy(caller, 'Calendar access')
        if (denied) return denied
        const name = caller.model
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
