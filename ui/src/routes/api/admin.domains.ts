import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { addOrgDomain, listOrgDomains, removeOrgDomain, verifyOrgDomain } from '@/server/org-domains'
import { logAudit } from '@/server/audit'

// Sign-up domains. GET → the list. POST { domain } → add (returns the TXT
// token to publish). POST { verifyId } → run the DNS check. DELETE { id } →
// remove (self-joins from it stop immediately). Admins only.
export const Route = defineApi('/api/admin/domains', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    return json({ domains: await listOrgDomains() })
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const actor = user.email ?? user.name ?? 'admin'
    const body = await parseBody(
      request,
      z.union([z.object({ domain: z.string().min(3).max(253) }), z.object({ verifyId: z.string().uuid() })]),
    )
    if (body instanceof Response) return body
    if ('verifyId' in body) {
      const r = await verifyOrgDomain(body.verifyId)
      if (r.verified) {
        void logAudit({ actor, action: 'domain.verify', targetType: 'org-domain', targetId: body.verifyId })
      }
      return json(r)
    }
    try {
      const domain = await addOrgDomain(body.domain, actor)
      void logAudit({ actor, action: 'domain.add', targetType: 'org-domain', targetId: domain.id, targetLabel: domain.domain })
      return json({ domain })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  DELETE: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, z.object({ id: z.string().uuid() }))
    if (body instanceof Response) return body
    await removeOrgDomain(body.id)
    void logAudit({
      actor: actorOf(user),
      action: 'domain.remove',
      targetType: 'org-domain',
      targetId: body.id,
    })
    return json({ ok: true })
  },
})
