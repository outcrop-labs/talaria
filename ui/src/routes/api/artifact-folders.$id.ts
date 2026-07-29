import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { deleteFolder, updateFolder } from '@/server/artifacts'

const Patch = z.object({
  name: z.string().min(1).max(80).optional(),
  icon: z.string().max(16).nullish(),
  parentId: z.string().uuid().nullish(),
})

// One artifact folder. PUT → rename / set icon / reparent. DELETE → remove
// (its artifacts + child folders fall back to the root).
export const Route = createFileRoute('/api/artifact-folders/$id')({
  server: {
    handlers: {
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'artifacts.create'))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const folder = await updateFolder(params.id, parsed.data)
        if (!folder) return json({ error: 'invalid' }, { status: 400 })
        return json({ folder })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await hasPerm(user, 'artifacts.create'))) return json({ error: 'forbidden' }, { status: 403 })
        await deleteFolder(params.id)
        return json({ ok: true })
      },
    },
  },
})
