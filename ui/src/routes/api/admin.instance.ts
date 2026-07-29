import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { getInstanceDomain, setInstanceDomain, verifyInstanceDomain } from '@/server/instance'
import { logAudit } from '@/server/audit'

// The instance's hosting domain. GET → current config. PUT { domain } → set
// (unverified until the round trip passes); { domain: null } clears.
// POST { verify: true } → run the self-fetch (the action). Admins only.
export const Route = createFileRoute('/api/admin/instance')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireAdmin(request)
        if (gate instanceof Response) return gate
        return json({ instance: await getInstanceDomain() })
      },
      PUT: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ domain: z.string().min(3).max(253).nullable() }))
        if (body instanceof Response) return body
        try {
          const instance = await setInstanceDomain(body.domain)
          void logAudit({ actor: actorOf(user), action: 'instance.domain_set', targetType: 'instance', targetId: 'domain', after: { domain: body.domain } })
          return json({ instance })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      POST: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, z.object({ verify: z.literal(true) }))
        if (body instanceof Response) return body
        const r = await verifyInstanceDomain()
        if (r.verified) void logAudit({ actor: actorOf(user), action: 'instance.domain_verify', targetType: 'instance', targetId: 'domain' })
        return json(r)
      },
    },
  },
})
