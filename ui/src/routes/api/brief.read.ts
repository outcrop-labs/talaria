import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { parseBody, requireUser } from '@/server/api-guard'
import { markBriefRead } from '@/server/daily-brief'

const Body = z.object({ briefId: Uuid, seq: z.number().int().min(0) })

/** Move the reader's cursor. The ONLY mutation this feature exposes — there is
 *  no edit, no dismiss and no delete, because the document is append-only and
 *  every one of those would be a rewrite wearing a different name. */
export const Route = defineApi('/api/brief/read', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    // Scoped to the caller inside `markBriefRead` (the update carries
    // `user_id = ${userId}`), so a brief id belonging to somebody else matches
    // no row rather than moving their cursor.
    await markBriefRead(user.id, body.briefId, body.seq)
    return json({ ok: true })
  },
})
