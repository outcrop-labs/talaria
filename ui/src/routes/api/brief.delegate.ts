import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { parseBody, requireUser } from '@/server/api-guard'
import { grantReply, listGrants, releaseDrafts, revokeReply } from '@/server/daily-brief-delegation'

const Body = z.object({
  /** Null grants across every conversation; an id grants one thread. */
  channelId: Uuid.nullable(),
  granted: z.boolean(),
})

/** Let the assistant answer without asking, or stop it.
 *
 *  OWNER-ONLY BY CONSTRUCTION. This is not an admin `Perm` — nobody can grant
 *  it on somebody else's behalf, and there is no route that takes a user id.
 *  `grantReply` re-checks channel membership rather than trusting this handler,
 *  because a grant on a conversation the owner is not in would let an agent
 *  speak in a room its owner cannot read. */
// doc: Grant or revoke the assistant's reply-without-asking privilege, org-wide
// doc: (null) or for one channel. Owner-only by construction: not a Perm, and no
// doc: route takes a user id — nobody can grant it on somebody else's behalf.

export const Route = defineApi('/api/brief/delegate', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({ grants: await listGrants(user.id) })
  },
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    if (!body.granted) return json({ revoked: await revokeReply(user.id, body.channelId) })
    const grant = await grantReply(user.id, body.channelId)
    if (!grant) return json({ error: 'That is not one of your conversations.' }, { status: 403 })
    // Granting permission to send a reply that is already written means sending
    // it — otherwise the control appears to do nothing until the other person
    // happens to speak again. Awaited, so the response reflects what happened.
    const sent = await releaseDrafts(user.id, body.channelId ?? undefined).catch(() => 0)
    return json({ grant, sent })
  },
})
