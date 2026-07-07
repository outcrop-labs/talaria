import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getPublicSpace } from '@/server/kb'

// Public folder read — no auth. Only spaces with visibility 'public' resolve;
// returns the folder's name + overview (its body), like a public doc.
export const Route = createFileRoute('/api/kb/public/space/$slug')({
  server: {
    handlers: {
      GET: async ({ params }) => {
        const space = await getPublicSpace(params.slug)
        if (!space) return json({ error: 'not found' }, { status: 404 })
        return json({ space: { name: space.name, icon: space.icon, body: space.body } })
      },
    },
  },
})
