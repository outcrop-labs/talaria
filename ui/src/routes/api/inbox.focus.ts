import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { listFocusQueue } from '@/server/inbox-focus'

export const Route = defineApi('/api/inbox/focus', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    return json(await listFocusQueue(user))
  },
})
