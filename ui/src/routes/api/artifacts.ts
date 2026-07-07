import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createArtifact, guarded, listArtifacts } from '@/server/artifacts'
import { canRead, grantedItemIds } from '@/server/kb-perms'

const Body = z.object({
  kind: z.enum(['doc', 'sheet', 'microsite', 'file']).optional(),
  title: z.string().max(200).optional(),
})

// Artifacts the caller can read. POST creates one (owned by the caller).
export const Route = createFileRoute('/api/artifacts')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const granted = await grantedItemIds('artifact', user.id)
        const artifacts = (await listArtifacts()).filter((a) => granted.has(a.id) || canRead(guarded(a), user.id, user.email ?? user.name))
        return json({ artifacts })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const artifact = await createArtifact({ kind: parsed.data.kind, title: parsed.data.title, createdBy: user.email ?? user.name ?? 'user', ownerUserId: user.id })
        return json({ artifact })
      },
    },
  },
})
