import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { ensureEffortsCatalog, effortsForModel } from '@/server/model-efforts'
import { personaConfiguredEffort } from '@/server/harness/persona'

// The composer's effort-picker feed: which reasoning-effort levels THIS model
// id may be asked for, plus the default it should start from. Thin by the house
// rule (routes parse and serialize; the decisions live in
// `server/model-efforts.ts` and `server/harness/persona.ts`) — the route adds
// only the auth gate and the query string.
//
// `model` accepts both spellings the chat surfaces speak: a fleet persona id
// (base or tier) or a gateway catalog id. `efforts` is `[]` when nothing
// vouches for any level, and `[]` is what the composer renders as "no picker".
// `default` is the AGENT-CONFIGURED effort when the id is a persona whose
// config names one — the pick an admin set beside the model in the agent
// editor, validated against the same levels; null everywhere else. Precedence
// at the surfaces: conversation pick > agent default > the user's platform
// default > the model's own.
//
// An empty first read runs the BACKFILL before answering: a catalog stored by
// a build before the effort extraction has no levels for anyone, and the
// picker would stay hidden until an admin happened to re-open the model
// modal. `ensureEffortsCatalog` refreshes the serving endpoints' catalogs
// (once — post-feature entries never re-trigger) and answers from the fresh
// store, so the first request after an upgrade may take a few seconds and
// every one after is a settings read.
export const Route = defineApi('/api/models/efforts', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const model = new URL(request.url).searchParams.get('model')?.trim()
    if (!model) return json({ error: 'model is required' }, { status: 400 })
    let efforts = await effortsForModel(model)
    if (efforts.length === 0) efforts = await ensureEffortsCatalog(model)
    if (efforts.length === 0) return json({ efforts: [], default: null })
    // The configured default, held against the levels just read: a level the
    // model no longer publishes (the admin swapped models, the metadata
    // changed) is not a default, it is a stale string.
    const configured = await personaConfiguredEffort(model).catch(() => null)
    return json({ efforts, default: configured && efforts.includes(configured) ? configured : null })
  },
})
