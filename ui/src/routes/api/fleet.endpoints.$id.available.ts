import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { listEndpoints } from '@/server/agent-defs'
import { availableModels } from '@/server/provider-catalog'

// GET → what this provider actually offers right now (live /models call,
// server-side, keys never leave the box). Admin.
export const Route = createFileRoute('/api/fleet/endpoints/$id/available')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const ep = (await listEndpoints()).find((e) => e.id === params.id)
        if (!ep) return json({ error: 'not found' }, { status: 404 })
        try {
          return json({ models: await availableModels(ep) })
        } catch (e) {
          return json({ models: [], note: (e as Error).message })
        }
      },
    },
  },
})
