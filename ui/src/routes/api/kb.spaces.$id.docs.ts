import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createDoc, getSpace, listDocs } from '@/server/kb'
import { canRead, grantedItemIds, listEditors } from '@/server/kb-perms'

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
        // Gate the whole tree on folder access first.
        const space = await getSpace(params.id)
        if (!space) return json({ docs: [] })
        if (!canRead(space, user.id, user.email ?? user.name, await listEditors('space', params.id))) return json({ docs: [] })
        // Inherited docs are as visible as the (readable) folder, so they show.
        // Customized docs are filtered by their own audience (or an explicit grant).
        const granted = await grantedItemIds('doc', user.id)
        const docs = (await listDocs(params.id)).filter(
          (d) => d.permsInherited || granted.has(d.id) || canRead(d, user.id, user.email ?? user.name),
        )
        return json({ docs })
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
            ownerUserId: user.id,
          }),
        })
      },
    },
  },
})
