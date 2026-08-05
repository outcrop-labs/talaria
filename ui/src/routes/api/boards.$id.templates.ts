import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardRole, canEdit } from '@/server/boards'
import { boardTemplates, setBoardTemplates } from '@/server/templates'

const Put = z.object({
  templateIds: z.array(z.string().uuid()).max(50),
  defaultId: z.string().uuid().nullable(),
})

// The ticket templates a board uses. GET → bindings. PUT { templateIds,
// defaultId } → replace the set (owner/editor); defaultId must be in the set.
export const Route = defineApi('/api/boards/$id/templates', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    return json({ bindings: await boardTemplates(params.id) })
  },
  PUT: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Put)
    if (body instanceof Response) return body
    if (body.defaultId && !body.templateIds.includes(body.defaultId)) {
      return json({ error: 'defaultId must be one of templateIds' }, { status: 400 })
    }
    await setBoardTemplates(params.id, body.templateIds, body.defaultId)
    return json({ bindings: await boardTemplates(params.id) })
  },
})
