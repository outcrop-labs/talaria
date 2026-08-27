import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { parseBody, requireUser } from '@/server/api-guard'
import { ensureDm } from '@/server/channels'

const Post = z.object({ userId: Uuid })

// POST { userId } → find-or-create the DM with that person (rides the channel
// machinery: same messages, SSE feed, and composer as everything else).
export const Route = defineApi('/api/dms', {
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
})
