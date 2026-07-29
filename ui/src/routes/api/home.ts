import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { homeSummary } from '@/server/home'

// The Home/Today summary for the signed-in user.
export const Route = createFileRoute('/api/home')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json(await homeSummary(user.id, user.role))
      },
    },
  },
})
