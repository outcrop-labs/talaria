import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { listUsersAdmin, setUserAgentAccess, setUserRole } from '@/server/users'

// Admin console API. GET → all users with roles + agent allow-lists.
// PUT { userId, role? , agentModels? } → update either. Admins only.
export const Route = createFileRoute('/api/admin/users')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ users: await listUsersAdmin() })
      },
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({
            userId: z.string().uuid(),
            role: z.enum(['admin', 'member']).optional(),
            agentModels: z.array(z.string().max(200)).max(100).optional(),
          })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // No self-demotion — you'd lock yourself out of this page.
        if (parsed.data.role === 'member' && parsed.data.userId === user.id) {
          return json({ error: 'you cannot demote yourself' }, { status: 400 })
        }
        if (parsed.data.role) await setUserRole(parsed.data.userId, parsed.data.role)
        if (parsed.data.agentModels) await setUserAgentAccess(parsed.data.userId, parsed.data.agentModels)
        return json({ ok: true })
      },
    },
  },
})
