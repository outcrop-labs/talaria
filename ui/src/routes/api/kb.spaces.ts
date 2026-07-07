import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createSpace, listSpaces } from '@/server/kb'
import { canRead, grantedItemIds } from '@/server/kb-perms'

const Body = z.object({ name: z.string().min(1).max(80), description: z.string().max(400).optional(), icon: z.string().max(8).optional() })

// KB spaces (any member). GET → all. POST → create.
export const Route = createFileRoute('/api/kb/spaces')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        // Hide folders the caller can't read, but keep ones shared with them.
        const granted = await grantedItemIds('space', user.id)
        const spaces = (await listSpaces()).filter((s) => granted.has(s.id) || canRead(s, user.id, user.email ?? user.name))
        return json({ spaces })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        return json({ space: await createSpace({ ...parsed.data, createdBy: user.email ?? user.name ?? 'user', ownerUserId: user.id }) })
      },
    },
  },
})
