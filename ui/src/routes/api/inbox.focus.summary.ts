import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { focusSummary } from '@/server/inbox-focus'

export const Route = createFileRoute('/api/inbox/focus/summary')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json(await focusSummary(user))
      },
    },
  },
})
