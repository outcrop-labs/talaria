import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { addOrgDomain, listOrgDomains, removeOrgDomain, verifyOrgDomain } from '@/server/org-domains'
import { logAudit } from '@/server/audit'

// Sign-up domains. GET → the list. POST { domain } → add (returns the TXT
// token to publish). POST { verifyId } → run the DNS check. DELETE { id } →
// remove (self-joins from it stop immediately). Admins only.
export const Route = createFileRoute('/api/admin/domains')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ domains: await listOrgDomains() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const actor = user.email ?? user.name ?? 'admin'
        const parsed = z
          .union([z.object({ domain: z.string().min(3).max(253) }), z.object({ verifyId: z.string().uuid() })])
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if ('verifyId' in parsed.data) {
          const r = await verifyOrgDomain(parsed.data.verifyId)
          if (r.verified) {
            void logAudit({ actor, action: 'domain.verify', targetType: 'org-domain', targetId: parsed.data.verifyId })
          }
          return json(r)
        }
        try {
          const domain = await addOrgDomain(parsed.data.domain, actor)
          void logAudit({ actor, action: 'domain.add', targetType: 'org-domain', targetId: domain.id, targetLabel: domain.domain })
          return json({ domain })
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
        await removeOrgDomain(parsed.data.id)
        void logAudit({
          actor: user.email ?? user.name ?? 'admin',
          action: 'domain.remove',
          targetType: 'org-domain',
          targetId: parsed.data.id,
        })
        return json({ ok: true })
      },
    },
  },
})
