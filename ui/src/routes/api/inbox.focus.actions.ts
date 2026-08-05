import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { runFocusAction } from '@/server/inbox-focus'
import { acquireInboxFocusLock, attachTimelineToActionResult } from '@/server/inbox-focus-conversation'

const Body = z.object({
  key: z.string().min(1).max(600).optional(),
  actionId: z.string().min(1).max(100).optional(),
  payload: z.unknown().optional(),
  commandDecisionId: z.string().uuid().optional(),
  decisionId: z.string().uuid().optional(),
  confirmationToken: z.string().min(20).max(200).optional(),
  cancelDecisionId: z.string().uuid().optional(),
  undoDecisionId: z.string().uuid().optional(),
})

export const Route = defineApi('/api/inbox/focus/actions', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const release = acquireInboxFocusLock(user.id)
    if (!release) return json({ status: 'failed', message: 'Your assistant is already handling another Inbox action.' }, { status: 409 })
    let result
    try {
      result = await attachTimelineToActionResult(user, await runFocusAction(user, body))
    } finally {
      release()
    }
    const status = result.status === 'stale' ? 409 : result.status === 'failed' ? 422 : 200
    return json(result, { status })
  },
})
