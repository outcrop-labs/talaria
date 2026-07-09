import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { createTeam, listTeams } from '@/server/teams'
import { actingUser } from '@/server/users'

// GET → the user's teams (humans, or a personal assistant acting as its owner).
// POST { name } → create a team (user becomes owner; humans only).
export const Route = createFileRoute('/api/teams')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await actingUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ teams: await listTeams(user.id) })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = z.object({ name: z.string().min(1).max(120) }).safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        return json({ team: await createTeam(user.id, parsed.data.name) })
      },
    },
  },
})
