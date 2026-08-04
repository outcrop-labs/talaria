import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { listFocusQueue } from '@/server/inbox-focus'

export const Route = createFileRoute('/api/inbox/focus')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        return json(await listFocusQueue(user))
      },
    },
  },
})
