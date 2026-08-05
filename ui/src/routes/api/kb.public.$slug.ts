import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getPublicDoc } from '@/server/kb'

// Public doc read — no auth. Only docs with visibility 'public' resolve.
export const Route = defineApi('/api/kb/public/$slug', {
  GET: async ({ params }) => {
    const doc = await getPublicDoc(params.slug)
    if (!doc) return json({ error: 'not found' }, { status: 404 })
    return json({ doc: { title: doc.title, body: doc.body, updatedAt: doc.updatedAt } })
  },
})
