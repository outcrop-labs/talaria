import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requireUser } from '@/server/api-guard'
import { acquireInboxFocusLock, getInboxConversation } from '@/server/inbox-focus-conversation'

export const Route = createFileRoute('/api/inbox/focus/conversation')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const cursor = new URL(request.url).searchParams.get('cursor')
        const release = acquireInboxFocusLock(user.id)
        if (!release) return json({ error: 'Your assistant is updating the Inbox conversation.' }, { status: 409 })
        try {
          return json(await getInboxConversation(user, cursor))
        } finally {
          release()
        }
      },
    },
  },
})
