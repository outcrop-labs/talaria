import { defineApi } from '@/server/api-route'
import { requireUser } from '@/server/api-guard'
import { userEventStream } from '@/server/realtime'

// GET /api/me/events → SSE stream of THIS person's own firehose: their runs
// changing state (and, when notifications learn to publish, their bell).
//
// The per-device attach point. A run started on a laptop and parked on a
// question at 11pm has to reach the phone, and it has to reach it without the
// phone knowing which run to watch — which is the one thing `run:<id>` cannot
// do. Events here are id-shaped (see `UserEvent`); the client re-fetches
// through the ordinary routes, so this stream widens nothing.
//
// AUTHORIZATION IS THE SESSION. The topic is keyed by `user.id` taken from the
// resolved session and never from the request — no `?userId=`, no param, no
// route segment to tamper with. There is deliberately no way to ask for
// somebody else's firehose, which is why this route needs no check beyond
// being signed in: the only id it can construct is the caller's own.
export const Route = defineApi('/api/me/events', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return new Response(userEventStream(user.id, request.signal), {
      headers: {
        'Content-Type': 'text/event-stream',
        'Cache-Control': 'no-cache, no-transform',
        Connection: 'keep-alive',
      },
    })
  },
})
