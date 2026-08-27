import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { FOCUS_SOURCE_TYPES, updateFocusState } from '@/server/inbox-focus'
import { acquireInboxFocusLock, recordInboxSnooze } from '@/server/inbox-focus-conversation'

const Body = z
  .object({
    sourceType: z.enum(FOCUS_SOURCE_TYPES),
    sourceId: z.string().min(1).max(500),
    snoozedUntil: z.string().datetime().nullable().optional(),
    viewed: z.boolean().optional(),
  })
  .refine((body) => body.snoozedUntil !== undefined || body.viewed, 'state change required')
// doc: Mark a focus item viewed, or snooze it until a time.


export const Route = defineApi('/api/inbox/focus/state', {
  PUT: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const release = acquireInboxFocusLock(user.id)
    if (!release) return json({ error: 'Your assistant is already handling another Inbox action.' }, { status: 409 })
    try {
      const updated = await updateFocusState(user, body)
      if (!updated) return json({ error: 'That focus item is no longer available.' }, { status: 409 })
      const timelineEntry = body.snoozedUntil
        ? await recordInboxSnooze(user, { sourceType: body.sourceType, sourceId: body.sourceId, snoozedUntil: body.snoozedUntil })
        : null
      return json({ ok: true, ...(timelineEntry ? { timelineEntry } : {}) })
    } finally {
      release()
    }
  },
})
