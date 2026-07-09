import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createTemplate, listTemplates } from '@/server/templates'

const Body = z.object({
  name: z.string().trim().min(1).max(120),
  kind: z.enum(['ticket', 'plan']),
  body: z.string().max(50_000).optional(),
  guidance: z.string().max(10_000).optional(),
})

// The org's template library (ticket + plan formats). GET → all (any member —
// the library grounds pickers everywhere). POST → create (any member, like
// boards/channels; the skeletons are org-shared working material, not policy).
export const Route = createFileRoute('/api/templates')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ templates: await listTemplates() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const template = await createTemplate({ ...parsed.data, createdBy: user.email ?? user.name ?? 'user' })
        return json({ template })
      },
    },
  },
})
