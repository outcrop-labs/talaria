import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requirePerm } from '@/server/api-guard'
import { deleteTemplate, getTemplate, updateTemplate } from '@/server/templates'

const Patch = z.object({
  name: z.string().trim().min(1).max(120).optional(),
  body: z.string().max(50_000).optional(),
  guidance: z.string().max(10_000).optional(),
})

// One template: PUT → edit (kind is immutable — retire and recreate instead),
// DELETE → remove (bindings cascade/null out; consumers fall through the chain).
export const Route = defineApi('/api/templates/$id', {
  PUT: async ({ request, params }) => {
    const user = await requirePerm(request, 'templates.manage')
    if (user instanceof Response) return user
    const body = await parseBody(request, Patch)
    if (body instanceof Response) return body
    const template = await updateTemplate(params.id, body, user.email ?? user.name)
    if (!template) return json({ error: 'not found' }, { status: 404 })
    return json({ template })
  },
  DELETE: async ({ request, params }) => {
    const user = await requirePerm(request, 'templates.manage')
    if (user instanceof Response) return user
    if (!(await getTemplate(params.id))) return json({ error: 'not found' }, { status: 404 })
    await deleteTemplate(params.id)
    return json({ ok: true })
  },
})
