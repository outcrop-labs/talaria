import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { focusSummary } from '@/server/inbox-focus'
// doc: The one-screen summary of the caller's focus state — what's queued,
// doc: what's in flight, what needs a decision.


export const Route = defineApi('/api/inbox/focus/summary', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json(await focusSummary(user))
  },
})
