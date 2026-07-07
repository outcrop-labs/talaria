import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getPublicArtifact } from '@/server/artifacts'

// Public artifact read — no auth. Only artifacts set to 'public' resolve.
export const Route = createFileRoute('/api/artifacts/public/$slug')({
  server: {
    handlers: {
      GET: async ({ params }) => {
        const a = await getPublicArtifact(params.slug)
        if (!a) return json({ error: 'not found' }, { status: 404 })
        return json({ artifact: { kind: a.kind, title: a.title, icon: a.icon, body: a.body, updatedAt: a.updatedAt } })
      },
    },
  },
})
