import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { createInboxConversation, listInboxConversations } from '@/server/inbox-focus-conversation'

// GET → the panel's chat picker. POST → start a fresh conversation instance.
// Segmentation is the context strategy: a new instance is how old context is
// shed, and it is the owner's choice to make (no budget imposes it).
export const Route = defineApi('/api/inbox/focus/conversations', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json({ conversations: await listInboxConversations(user.id) })
  },
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const id = await createInboxConversation(user.id, null)
    return json({ conversation: { id } }, { status: 201 })
  },
})
