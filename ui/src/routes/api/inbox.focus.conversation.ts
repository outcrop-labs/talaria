import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { acquireInboxFocusLock, getInboxConversation } from '@/server/inbox-focus-conversation'

export const Route = defineApi('/api/inbox/focus/conversation', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const url = new URL(request.url)
    const cursor = url.searchParams.get('cursor')
    const conversationId = url.searchParams.get('conversationId')
    const release = acquireInboxFocusLock(user.id)
    if (!release) return json({ error: 'Your assistant is updating the Inbox conversation.' }, { status: 409 })
    try {
      return json(await getInboxConversation(user, cursor, conversationId))
    } finally {
      release()
    }
  },
})
