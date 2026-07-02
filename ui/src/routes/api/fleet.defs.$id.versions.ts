import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { getAgentDef, listVersions } from '@/server/agent-defs'

// GET → an agent definition's full version history (admin).
export const Route = createFileRoute('/api/fleet/defs/$id/versions')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })
        return json({ def, versions: await listVersions(params.id) })
      },
    },
  },
})
