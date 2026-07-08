import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createEvent, listUpcomingEvents } from '@/server/google/calendar'

const CreateBody = z.object({
  summary: z.string().min(1).max(500),
  description: z.string().max(8000).optional(),
  location: z.string().max(500).optional(),
  start: z.string().min(4),
  end: z.string().min(4),
  allDay: z.boolean().optional(),
  attendees: z.array(z.string().email()).max(50).optional(),
})

function fail(err: Error) {
  if (err.name === 'GoogleNotConnected') {
    return json({ error: 'not_connected', message: 'Connect a Google account first.' }, { status: 409 })
  }
  if (/insufficient|ACCESS_TOKEN_SCOPE/i.test(err.message)) {
    return json({ error: 'reconnect_needed', message: 'Reconnect Google to grant Calendar access.' }, { status: 409 })
  }
  if (import.meta.env.DEV) console.error('[calendar/events] failed:', err)
  return json({ error: 'calendar_error', message: 'Could not reach Google Calendar.' }, { status: 502 })
}

// GET  → upcoming events on the user's primary calendar
// POST → create an event
export const Route = createFileRoute('/api/integrations/google/calendar/events')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        try {
          return json({ events: await listUpcomingEvents(user.id, Date.now()) })
        } catch (err) {
          return fail(err as Error)
        }
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = CreateBody.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          return json({ event: await createEvent(user.id, Date.now(), parsed.data) })
        } catch (err) {
          return fail(err as Error)
        }
      },
    },
  },
})
