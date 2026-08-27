import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { parseBody, requireUser } from '@/server/api-guard'
import { decideDraft } from '@/server/daily-brief-delegation'

const Body = z.object({ draftId: Uuid, decision: z.enum(['approve', 'reject']) })

/** Send or discard a reply the assistant drafted.
 *
 *  The one route in this feature that causes something to LEAVE — a message
 *  another person receives — so it is the one that refuses on staleness. Every
 *  authority check lives in `decideDraft`, scoped to the caller's own drafts,
 *  rather than here: a second copy of that rule beside the first is how the
 *  two come to disagree. */
export const Route = defineApi('/api/brief/reply', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const outcome = await decideDraft(user.id, body.draftId, body.decision)
    if (outcome.status === 'gone') return json({ error: 'That draft is no longer available.' }, { status: 404 })
    // 409, not 400: the request was well-formed and was correct when the person
    // read it. The world moved underneath it, and the client's answer is to ask
    // for a fresh draft rather than to fix its input.
    if (outcome.status === 'stale') return json({ error: outcome.message }, { status: 409 })
    return json({ status: outcome.status })
  },
})
