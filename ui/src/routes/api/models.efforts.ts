import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { effortsForModel } from '@/server/model-efforts'

// The composer's effort-picker feed: which reasoning-effort levels THIS model
// id may be asked for. Thin by the house rule (routes parse and serialize; the
// decision lives in `server/model-efforts.ts`) — the route adds only the auth
// gate and the query string.
//
// `model` accepts both spellings the chat surfaces speak: a fleet persona id
// (base or tier) or a gateway catalog id. The answer is `[]` when nothing
// vouches for any level, and `[]` is what the composer renders as "no picker".
export const Route = defineApi('/api/models/efforts', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const model = new URL(request.url).searchParams.get('model')?.trim()
    if (!model) return json({ error: 'model is required' }, { status: 400 })
    return json({ efforts: await effortsForModel(model) })
  },
})
