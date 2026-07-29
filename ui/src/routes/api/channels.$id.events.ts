import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { channelRole } from '@/server/channels'
import { channelEventStream } from '@/server/realtime'

// GET /api/channels/:id/events → SSE stream of the channel's live events
// (messages, membership). Auth-gated to members. Powers multiplayer chat.
export const Route = createFileRoute('/api/channels/$id/events')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return new Response(channelEventStream(params.id, request.signal), {
          headers: {
            'Content-Type': 'text/event-stream',
            'Cache-Control': 'no-cache, no-transform',
            Connection: 'keep-alive',
          },
        })
      },
    },
  },
})
