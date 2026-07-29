import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { getInstanceDomain, setInstanceDomain, verifyInstanceDomain } from '@/server/instance'
import { logAudit } from '@/server/audit'

// The instance's hosting domain. GET → current config. POST { domain } → set
// (unverified until the round trip passes); { domain: null } clears;
// { verify: true } → run the self-fetch. Admins only.
export const Route = createFileRoute('/api/admin/instance')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ instance: await getInstanceDomain() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const actor = user.email ?? user.name ?? 'admin'
        const parsed = z
          .union([z.object({ domain: z.string().min(3).max(253).nullable() }), z.object({ verify: z.literal(true) })])
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if ('verify' in parsed.data) {
          const r = await verifyInstanceDomain()
          if (r.verified) void logAudit({ actor, action: 'instance.domain_verify', targetType: 'instance', targetId: 'domain' })
          return json(r)
        }
        try {
          const instance = await setInstanceDomain(parsed.data.domain)
          void logAudit({ actor, action: 'instance.domain_set', targetType: 'instance', targetId: 'domain', after: { domain: parsed.data.domain } })
          return json({ instance })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
