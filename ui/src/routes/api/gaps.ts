import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { listGaps } from '@/server/gaps'

// The Studio's Suggested queue: capability gaps agents have reported, ranked
// by how often the work-shape recurs. Any member reads (the queue is what
// invites people to tailor); status changes live on /api/gaps/$id.
export const Route = defineApi('/api/gaps', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const url = new URL(request.url)
    const status = url.searchParams.get('status') ?? undefined
    return json({ gaps: await listGaps(status) })
  },
})
