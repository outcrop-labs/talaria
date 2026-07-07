import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { deleteSpace, getSpace, updateSpace } from '@/server/kb'
import { logAudit } from '@/server/audit'

const Patch = z.object({
  name: z.string().min(1).max(80).optional(),
  description: z.string().max(400).nullish(),
  icon: z.string().max(16).nullish(),
  body: z.string().max(500_000).optional(),
})

// One KB space. PUT → rename / set icon. DELETE → remove it and its docs
// (official docs are unindexed from the org brain first).
export const Route = createFileRoute('/api/kb/spaces/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const space = await getSpace(params.id)
        if (!space) return json({ error: 'not found' }, { status: 404 })
        return json({ space })
      },
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const space = await updateSpace(params.id, parsed.data)
        if (!space) return json({ error: 'not found' }, { status: 404 })
        return json({ space })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        await deleteSpace(params.id)
        void logAudit({ actor: user.email ?? user.name ?? 'user', action: 'kb.space.delete', targetType: 'kb-space', targetId: params.id })
        return json({ ok: true })
      },
    },
  },
})
