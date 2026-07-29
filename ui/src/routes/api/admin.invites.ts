import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createInvite, listInvites, revokeInvite } from '@/server/invites'
import { logAudit } from '@/server/audit'

// Invites. GET → recent invites with state. POST { email } → create + send
// (re-invites re-issue with a fresh token). DELETE { id } → revoke.
export const Route = createFileRoute('/api/admin/invites')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ invites: await listInvites() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const actor = user.email ?? user.name ?? 'admin'
        const parsed = z.object({ email: z.string().max(200) }).safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          const r = await createInvite(parsed.data.email, actor, new URL(request.url).origin)
          void logAudit({ actor, action: 'invite.create', targetType: 'invite', targetId: r.invite.id, targetLabel: r.invite.email })
          return json(r)
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      DELETE: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z.object({ id: z.string().uuid() }).safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        await revokeInvite(parsed.data.id)
        void logAudit({ actor: user.email ?? user.name ?? 'admin', action: 'invite.revoke', targetType: 'invite', targetId: parsed.data.id })
        return json({ ok: true })
      },
    },
  },
})
