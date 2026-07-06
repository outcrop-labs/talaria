import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createDoc, listDocs } from '@/server/kb'

const Body = z.object({
  title: z.string().max(200).optional(),
  parentId: z.string().uuid().nullish(),
  kind: z.enum(['human', 'agent']).optional(),
})

// A space's docs (tree). GET → doc metadata list. POST → new doc.
export const Route = createFileRoute('/api/kb/spaces/$id/docs')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ docs: await listDocs(params.id) })
      },
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        return json({
          doc: await createDoc({
            spaceId: params.id,
            parentId: parsed.data.parentId ?? null,
            title: parsed.data.title,
            kind: parsed.data.kind,
            createdBy: user.email ?? user.name ?? 'user',
          }),
        })
      },
    },
  },
})
