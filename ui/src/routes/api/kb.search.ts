import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { searchDocs } from '@/server/kb'

// Full-text search across the knowledgebase (docs the caller can read).
export const Route = defineApi('/api/kb/search', {
  GET: async ({ request }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const q = new URL(request.url).searchParams.get('q') ?? ''
    return json({ hits: await searchDocs(q, { userId: user.id, who: user.email ?? user.name }) })
  },
})
