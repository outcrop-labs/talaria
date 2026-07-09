import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { listAgentDefs, listEndpoints } from '@/server/agent-defs'
import { fleetBrainHealth } from '@/server/brain-health'

// The harness registry. GET → agent definitions (latest version inline) +
// LLM endpoints. Admins only — the config surface includes infra layout.
export const Route = createFileRoute('/api/fleet/defs')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        return json({ defs: await listAgentDefs(), endpoints: await listEndpoints(), brains: await fleetBrainHealth() })
      },
    },
  },
})
