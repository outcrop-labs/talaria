import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { getBacklinks } from '@/server/kb'

// Docs that link to this one ("linked from"). Editor links point at
// /knowledge/<id>, so backlinks fall out of a substring match.
export const Route = createFileRoute('/api/kb/docs/$id/backlinks')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        return json({ backlinks: await getBacklinks(params.id) })
      },
    },
  },
})
