import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { createInvite, listInvites, revokeInvite } from '@/server/invites'
import { logAudit } from '@/server/audit'

// Invites. GET → recent invites with state. POST { email } → create + send
// (re-invites re-issue with a fresh token). DELETE { id } → revoke.
export const Route = defineApi('/api/admin/invites', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    return json({ invites: await listInvites() })
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const actor = user.email ?? user.name ?? 'admin'
    const body = await parseBody(request, z.object({ email: z.string().max(200) }))
    if (body instanceof Response) return body
    try {
      const r = await createInvite(body.email, actor, new URL(request.url).origin)
      void logAudit({ actor, action: 'invite.create', targetType: 'invite', targetId: r.invite.id, targetLabel: r.invite.email })
      return json(r)
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  DELETE: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, z.object({ id: z.string().uuid() }))
    if (body instanceof Response) return body
    await revokeInvite(body.id)
    void logAudit({ actor: actorOf(user), action: 'invite.revoke', targetType: 'invite', targetId: body.id })
    return json({ ok: true })
  },
})
