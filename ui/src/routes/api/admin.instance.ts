import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { getInstanceDomain, setInstanceDomain, verifyInstanceDomain } from '@/server/instance'
import { logAudit } from '@/server/audit'

// The instance's hosting domain. GET → current config. POST { domain } → set
// (unverified until the round trip passes); { domain: null } clears;
// { verify: true } → run the self-fetch. Admins only.
export const Route = createFileRoute('/api/admin/instance')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireAdmin(request)
        if (gate instanceof Response) return gate
        return json({ instance: await getInstanceDomain() })
      },
      POST: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const actor = actorOf(user)
        const body = await parseBody(
          request,
          z.union([z.object({ domain: z.string().min(3).max(253).nullable() }), z.object({ verify: z.literal(true) })]),
        )
        if (body instanceof Response) return body
        if ('verify' in body) {
          const r = await verifyInstanceDomain()
          if (r.verified) void logAudit({ actor, action: 'instance.domain_verify', targetType: 'instance', targetId: 'domain' })
          return json(r)
        }
        try {
          const instance = await setInstanceDomain(body.domain)
          void logAudit({ actor, action: 'instance.domain_set', targetType: 'instance', targetId: 'domain', after: { domain: body.domain } })
          return json({ instance })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
