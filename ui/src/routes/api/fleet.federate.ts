import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { federateFromDir } from '@/server/fleet-federate'
import { logAudit } from '@/server/audit'

const Body = z.object({
  /** Server-side path to a Hermes-format directory (admin trust model). */
  dir: z.string().trim().min(1).max(500),
})

// POST → federate outside agents into Talaria: read a Hermes-format directory
// and create each agent natively (Talaria def, fresh key + state volume, our
// chassis, skills copied in). One-way and re-runnable. Admin.
export const Route = createFileRoute('/api/fleet/federate')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const actor = user.email ?? user.name ?? 'admin'
        const result = await federateFromDir(parsed.data.dir, actor)
        if (result.agents.some((a) => a.status === 'federated')) {
          void logAudit({ actor, action: 'agent.federate', targetType: 'fleet', targetId: 'fleet', targetLabel: parsed.data.dir })
        }
        return json({ result })
      },
    },
  },
})
