import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { archiveInboxConversation } from '@/server/inbox-focus-conversation'

// DELETE → archive one conversation instance. Scoped inside the module to the
// caller's own inbox conversations, so an id from the picker can never touch
// one of their ordinary chats. Archiving, not deleting: the messages and the
// decision timeline are the owner's record of what their assistant did.
export const Route = defineApi('/api/inbox/focus/conversations/$id', {
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const ok = await archiveInboxConversation(user.id, params.id)
    if (!ok) return json({ error: 'no such conversation' }, { status: 404 })
    return json({ ok: true })
  },
})
