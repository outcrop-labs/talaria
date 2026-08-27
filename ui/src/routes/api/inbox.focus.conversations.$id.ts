import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { acquireInboxFocusLock, archiveInboxConversation, getInboxConversation } from '@/server/inbox-focus-conversation'

// One conversation instance, by its path id. GET → its timeline page (cursor
// paginates); DELETE → archive it. Both are scoped inside the module to the
// caller's own inbox conversations, so an id from the picker can never touch
// one of their ordinary chats. Archiving, not deleting: the messages and the
// decision timeline are the owner's record of what their assistant did.
//
// GET holds the assistant lock while it reads: the timeline must not be
// served half-way through the assistant's own write to it.
export const Route = defineApi('/api/inbox/focus/conversations/$id', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const url = new URL(request.url)
    const cursor = url.searchParams.get('cursor')
    // `current` is the panel's first load, which has no id to name — the
    // server resolves the caller's own latest instance. (DELETE never sees
    // it: no instance is named 'current', so it 404s like any other miss.)
    const release = acquireInboxFocusLock(user.id)
    if (!release) return json({ error: 'Your assistant is updating the Inbox conversation.' }, { status: 409 })
    try {
      return json(await getInboxConversation(user, cursor, params.id === 'current' ? null : params.id))
    } finally {
      release()
    }
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const ok = await archiveInboxConversation(user.id, params.id)
    if (!ok) return json({ error: 'no such conversation' }, { status: 404 })
    return json({ ok: true })
  },
})
