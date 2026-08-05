import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requirePerm } from '@/server/api-guard'
import { deleteFolder, updateFolder } from '@/server/artifacts'

const Patch = z.object({
  name: z.string().min(1).max(80).optional(),
  icon: z.string().max(16).nullish(),
  parentId: z.string().uuid().nullish(),
})

// One artifact folder. PUT → rename / set icon / reparent. DELETE → remove
// (its artifacts + child folders fall back to the root).
export const Route = defineApi('/api/artifact-folders/$id', {
  PUT: async ({ request, params }) => {
    const gate = await requirePerm(request, 'artifacts.create')
    if (gate instanceof Response) return gate
    const body = await parseBody(request, Patch)
    if (body instanceof Response) return body
    const folder = await updateFolder(params.id, body)
    if (!folder) return json({ error: 'invalid' }, { status: 400 })
    return json({ folder })
  },
  DELETE: async ({ request, params }) => {
    const gate = await requirePerm(request, 'artifacts.create')
    if (gate instanceof Response) return gate
    await deleteFolder(params.id)
    return json({ ok: true })
  },
})
