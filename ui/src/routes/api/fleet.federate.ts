import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { federateFromDir } from '@/server/fleet-federate'
import { logAudit } from '@/server/audit'

const Body = z.object({
  /** Server-side path to a Hermes-format directory (admin trust model). */
  dir: z.string().trim().min(1).max(500),
})

// POST → federate outside agents into Talaria: read a Hermes-format directory
// and create each agent natively (Talaria def, fresh key + state volume, our
// chassis, skills copied in). One-way and re-runnable. Admin.
export const Route = defineApi('/api/fleet/federate', {
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const actor = user.email ?? user.name ?? 'admin'
    const result = await federateFromDir(body.dir, actor)
    if (result.agents.some((a) => a.status === 'federated')) {
      void logAudit({ actor: actorOf(user), action: 'agent.federate', targetType: 'fleet', targetId: 'fleet', targetLabel: body.dir })
    }
    return json({ result })
  },
})
