import { z } from 'zod'
import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { parseBody, requireUser } from '@/server/api-guard'
import { markBriefItem } from '@/server/daily-brief'

const Body = z.object({
  sourceKey: z.string().min(1).max(200),
  action: z.enum(['check', 'dismiss', 'restore']),
  /** The reader's IANA zone — the check-off must land on the brief they are
   *  LOOKING at, which the fallback read may have served across a UTC
   *  midnight their timezone has not reached. */
  tz: z.string().max(64).nullable().optional(),
})

/** The owner's own verdict on a line: done, not needed, or put it back.
 *
 *  Scoped to the caller's own brief inside `markBriefItem` — there is no route
 *  that takes a user id, so a key belonging to somebody else's day resolves to
 *  no line rather than to theirs. */
export const Route = defineApi('/api/brief/item', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const result = await markBriefItem(user, body.sourceKey, body.action, new Date(), body.tz ?? null)
    // 404 rather than 400: the request was well formed, the line just is not on
    // today's page — usually a stale tab from yesterday's brief.
    if (!result.ok) return json({ error: result.reason ?? 'could not update that line' }, { status: 404 })
    return json({ ok: true })
  },
})
