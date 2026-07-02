import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser, updateSessionUser } from '@/server/auth/session'
import { setUserName } from '@/server/users'

// PUT /api/profile { name } → set the signed-in user's display name (updates
// the users row and the live session, so it shows everywhere immediately).
export const Route = createFileRoute('/api/profile')({
  server: {
    handlers: {
      PUT: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z
          .object({ name: z.string().min(1).max(80) })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const name = parsed.data.name.trim()
        await setUserName(user.id, name)
        const updated = await updateSessionUser(request, { name })
        return json({ user: updated })
      },
    },
  },
})
