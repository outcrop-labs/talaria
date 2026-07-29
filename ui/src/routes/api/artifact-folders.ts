import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requirePerm, requireUser } from '@/server/api-guard'
import { createFolder, listFolders } from '@/server/artifacts'

const Body = z.object({ name: z.string().min(1).max(80), parentId: z.string().uuid().nullish() })

// Artifact folders (org-wide organizational tree). GET → all. POST → create.
export const Route = createFileRoute('/api/artifact-folders')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const gate = await requireUser(request)
        if (gate instanceof Response) return gate
        return json({ folders: await listFolders() })
      },
      POST: async ({ request }) => {
        // Folders shape the org-wide artifact tree — same perm as creating
        // artifacts themselves.
        const gate = await requirePerm(request, 'artifacts.create')
        if (gate instanceof Response) return gate
        const user = gate
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        return json({ folder: await createFolder({ name: body.name, parentId: body.parentId ?? null, createdBy: user.email ?? user.name ?? 'user' }) })
      },
    },
  },
})
