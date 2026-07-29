import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { createFolder, listFolders } from '@/server/artifacts'

const Body = z.object({ name: z.string().min(1).max(80), parentId: z.string().uuid().nullish() })

// Artifact folders (org-wide organizational tree). GET → all. POST → create.
export const Route = createFileRoute('/api/artifact-folders')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ folders: await listFolders() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        // Folders shape the org-wide artifact tree — same perm as creating
        // artifacts themselves.
        if (!(await hasPerm(user, 'artifacts.create'))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        return json({ folder: await createFolder({ name: parsed.data.name, parentId: parsed.data.parentId ?? null, createdBy: user.email ?? user.name ?? 'user' }) })
      },
    },
  },
})
