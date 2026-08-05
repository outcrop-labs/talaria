import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getPublicArtifact } from '@/server/artifacts'

// Public artifact read — no auth. Only artifacts set to 'public' resolve.
export const Route = defineApi('/api/artifacts/public/$slug', {
  GET: async ({ params }) => {
    const a = await getPublicArtifact(params.slug)
    if (!a) return json({ error: 'not found' }, { status: 404 })
    return json({ artifact: { kind: a.kind, title: a.title, icon: a.icon, body: a.body, updatedAt: a.updatedAt } })
  },
})
