import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { requirePerm } from '@/server/api-guard'
import { listAgentDefs, listEndpoints } from '@/server/agent-defs'
import { fleetBrainHealth } from '@/server/brain-health'

// The harness registry. GET → agent definitions (latest version inline) +
// LLM endpoints. Admins only — the config surface includes infra layout.
export const Route = createFileRoute('/api/fleet/defs')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        return json({ defs: await listAgentDefs(), endpoints: await listEndpoints(), brains: await fleetBrainHealth() })
      },
    },
  },
})
