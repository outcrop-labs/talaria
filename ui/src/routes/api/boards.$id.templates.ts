import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { boardRole, canEdit } from '@/server/boards'
import { boardTemplates, setBoardTemplates } from '@/server/templates'

const Put = z.object({
  templateIds: z.array(z.string().uuid()).max(50),
  defaultId: z.string().uuid().nullable(),
})

// The ticket templates a board uses. GET → bindings. PUT { templateIds,
// defaultId } → replace the set (owner/editor); defaultId must be in the set.
export const Route = createFileRoute('/api/boards/$id/templates')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return json({ bindings: await boardTemplates(params.id) })
      },
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Put.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        if (parsed.data.defaultId && !parsed.data.templateIds.includes(parsed.data.defaultId)) {
          return json({ error: 'defaultId must be one of templateIds' }, { status: 400 })
        }
        await setBoardTemplates(params.id, parsed.data.templateIds, parsed.data.defaultId)
        return json({ bindings: await boardTemplates(params.id) })
      },
    },
  },
})
