import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { ensureDm } from '@/server/channels'

const Post = z.object({ userId: z.string().uuid() })

// POST { userId } → find-or-create the DM with that person (rides the channel
// machinery: same messages, SSE feed, and composer as everything else).
export const Route = createFileRoute('/api/dms')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, Post)
        if (body instanceof Response) return body
        try {
          return json({ channel: await ensureDm(user.id, body.userId) })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
