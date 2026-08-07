import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireAdmin } from '@/server/api-guard'
import { listEndpoints } from '@/server/agent-defs'
import { catalogModels } from '@/server/provider-catalog'
import { refreshEndpointCatalog } from '@/server/model-catalog'

// GET → what this provider actually offers right now (live /models call,
// server-side, keys never leave the box). Admin.
//
// THE CALL IS ALREADY BEING MADE, so it also refreshes the stored catalog: an
// admin opening the model picker is the moment the provider's own answer is
// freshest and the moment its descriptive fields are most worth keeping. Before
// this, the same request fetched four hundred rows of context windows, supported
// parameters and modalities, projected them to `string[]`, and dropped the rest
// on the floor — which is why the fitness page had to ask an admin to buy probes
// for facts the provider publishes for free.
export const Route = defineApi('/api/fleet/endpoints/$id/available', {
  GET: async ({ request, params }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const ep = (await listEndpoints()).find((e) => e.id === params.id)
    if (!ep) return json({ error: 'not found' }, { status: 404 })
    try {
      const models = await catalogModels(ep)
      // Storing is a side effect of answering and must never be able to fail the
      // answer: the picker's job is to list models, and an `app_settings` blip
      // is not a reason to show an admin an empty dropdown.
      void refreshEndpointCatalog(ep, { fetchCatalog: async () => models }).catch(() => {})
      // The ids stay the contract this route has always had; the descriptive
      // fields ride along for the picker that wants to show a window or a price.
      return json({ models: models.map((m) => m.id), catalog: models })
    } catch (e) {
      return json({ models: [], note: (e as Error).message })
    }
  },
})
