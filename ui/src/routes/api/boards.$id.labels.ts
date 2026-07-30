import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { boardRole, canEdit } from '@/server/boards'
import { createLabel, deleteLabel, listLabels, updateLabel } from '@/server/labels'

// Board labels. GET → the registry (any member). POST create, PUT rename/
// recolor (rename cascades into tickets), DELETE (strips off tickets) —
// owner/editor.
export const Route = createFileRoute('/api/boards/$id/labels')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        return json({ labels: await listLabels(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, z.object({ name: z.string().min(1).max(40), color: z.string().max(20).optional() }))
        if (body instanceof Response) return body
        try {
          return json({ label: await createLabel(params.id, body.name, body.color) })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      PUT: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(
          request,
          z.object({ labelId: z.string().uuid(), name: z.string().min(1).max(40).optional(), color: z.string().max(20).optional() }),
        )
        if (body instanceof Response) return body
        try {
          await updateLabel(params.id, body.labelId, { name: body.name, color: body.color })
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      DELETE: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        if (!canEdit(await boardRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
        const body = await parseBody(request, z.object({ labelId: z.string().uuid() }))
        if (body instanceof Response) return body
        await deleteLabel(params.id, body.labelId)
        return json({ ok: true })
      },
    },
  },
})
