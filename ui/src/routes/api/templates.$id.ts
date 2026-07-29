import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { deleteTemplate, getTemplate, updateTemplate } from '@/server/templates'

const Patch = z.object({
  name: z.string().trim().min(1).max(120).optional(),
  body: z.string().max(50_000).optional(),
  guidance: z.string().max(10_000).optional(),
})

// One template: PUT → edit (kind is immutable — retire and recreate instead),
// DELETE → remove (bindings cascade/null out; consumers fall through the chain).
export const Route = createFileRoute('/api/templates/$id')({
  server: {
    handlers: {
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'templates.manage'))) return json({ error: 'no permission to manage templates' }, { status: 403 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const template = await updateTemplate(params.id, parsed.data, user.email ?? user.name)
        if (!template) return json({ error: 'not found' }, { status: 404 })
        return json({ template })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'templates.manage'))) return json({ error: 'no permission to manage templates' }, { status: 403 })
        if (!(await getTemplate(params.id))) return json({ error: 'not found' }, { status: 404 })
        await deleteTemplate(params.id)
        return json({ ok: true })
      },
    },
  },
})
