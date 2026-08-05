import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requirePerm } from '@/server/api-guard'
import { listAgentDefs, listEndpoints } from '@/server/agent-defs'
import { fleetBrainHealth } from '@/server/brain-health'

// The harness registry. GET → agent definitions (latest version inline) +
// LLM endpoints. Admins only — the config surface includes infra layout.
export const Route = defineApi('/api/fleet/defs', {
  GET: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    return json({ defs: await listAgentDefs(), endpoints: await listEndpoints(), brains: await fleetBrainHealth() })
  },
})
