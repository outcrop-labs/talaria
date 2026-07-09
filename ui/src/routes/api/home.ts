import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { homeSummary } from '@/server/home'

// The Home/Today summary for the signed-in user.
export const Route = createFileRoute('/api/home')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json(await homeSummary(user.id, user.role))
      },
    },
  },
})
