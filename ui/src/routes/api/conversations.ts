import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { listConversations } from '@/server/conversations'

// GET /api/conversations → the current user's conversations (newest first).
// The client groups them by agent.
export const Route = createFileRoute('/api/conversations')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const kind = new URL(request.url).searchParams.get('kind') === 'plan' ? 'plan' : 'chat'
        return json({ conversations: await listConversations(user.id, kind) })
      },
    },
  },
})
