import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { ensureDm } from '@/server/channels'

// POST { userId } → find-or-create the DM with that person (rides the channel
// machinery: same messages, SSE feed, and composer as everything else).
export const Route = createFileRoute('/api/dms')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z.object({ userId: z.string().uuid() }).safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          return json({ channel: await ensureDm(user.id, parsed.data.userId) })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
