import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireAdmin } from '@/server/api-guard'
import { listEndpoints } from '@/server/agent-defs'
import { availableModels } from '@/server/provider-catalog'

// GET → what this provider actually offers right now (live /models call,
// server-side, keys never leave the box). Admin.
export const Route = defineApi('/api/fleet/endpoints/$id/available', {
  GET: async ({ request, params }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const ep = (await listEndpoints()).find((e) => e.id === params.id)
    if (!ep) return json({ error: 'not found' }, { status: 404 })
    try {
      return json({ models: await availableModels(ep) })
    } catch (e) {
      return json({ models: [], note: (e as Error).message })
    }
  },
})
