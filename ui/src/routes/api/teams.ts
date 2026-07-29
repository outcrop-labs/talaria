import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { createTeam, listTeams } from '@/server/teams'
import { actingUser } from '@/server/users'

const Post = z.object({ name: z.string().min(1).max(120) })

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
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, Post)
        if (body instanceof Response) return body
        return json({ team: await createTeam(user.id, body.name) })
      },
    },
  },
})
