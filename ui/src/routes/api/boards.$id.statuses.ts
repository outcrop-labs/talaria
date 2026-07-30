import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardRole, canEdit } from '@/server/boards'
import { createStatus, deleteStatus, listStatuses, reorderStatuses, updateStatus } from '@/server/statuses'

// Board statuses (custom workflow columns). GET → the ordered list incl. the
// system Blocked column (any member). POST create, PUT update/reorder, DELETE
// (tickets reassigned) — owner/editor. Category + agentStart carry the
// workflow semantics; Blocked is system and not editable here.
const Category = z.enum(['open', 'active', 'review', 'done'])

export const Route = createFileRoute('/api/boards/$id/statuses')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return json({ statuses: await listStatuses(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(
          request,
          z.object({
            label: z.string().min(1).max(40),
            color: z.string().max(20).optional(),
            category: Category.optional(),
            agentStart: z.boolean().optional(),
          }),
        )
        if (body instanceof Response) return body
        return json({ status: await createStatus(params.id, body) })
      },
      PUT: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(
          request,
          z.union([
            z.object({
              statusId: z.string().uuid(),
              label: z.string().min(1).max(40).optional(),
              color: z.string().max(20).optional(),
              category: Category.optional(),
              agentStart: z.boolean().optional(),
            }),
            z.object({ order: z.array(z.string().uuid()).min(1).max(50) }),
          ]),
        )
        if (body instanceof Response) return body
        try {
          if ('order' in body) await reorderStatuses(params.id, body.order)
          else await updateStatus(params.id, body.statusId, body)
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, z.object({ statusId: z.string().uuid(), reassignTo: z.string().max(40) }))
        if (body instanceof Response) return body
        try {
          await deleteStatus(params.id, body.statusId, body.reassignTo)
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
