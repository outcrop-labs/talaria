import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { getAgentDef, updateAgentMeta } from '@/server/agent-defs'

const Body = z.object({
  role: z.string().max(80).nullish(),
  displayName: z.string().min(1).max(80).optional(),
})

// PATCH → editable agent identity metadata (role, display name). Not versioned
// — this is identity, not config. Admin only.
export const Route = createFileRoute('/api/fleet/defs/$id')({
  server: {
    handlers: {
      PATCH: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if (!(await getAgentDef(params.id))) return json({ error: 'not found' }, { status: 404 })
        await updateAgentMeta(params.id, { role: parsed.data.role, displayName: parsed.data.displayName })
        return json({ ok: true })
      },
    },
  },
})
