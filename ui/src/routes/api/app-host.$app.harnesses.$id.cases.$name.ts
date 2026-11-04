import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireInternal } from '@/server/internal-guard'
import { listAppHarnesses, replayAppCase } from '@/server/app-harnesses'
import { slugOk } from '@/server/app-build/paths'

const MAX_MODEL = 300

// doc: POST /api/app-host/<slug>/harnesses/<id>/cases/<name> — replay one of an
// doc: enabled app harness's eval fixtures with the model pinned to `model`,
// doc: through the real runner with `harness_runs` and guard-findings filing
// doc: off (the sweep must not write into what it is compared against). Answers
// doc: the value, the model, the fixture's verdict (`pass`, `fail`, `gap`, or
// doc: null when there was no value to grade) and the run's contract, repair,
// doc: latency and token figures. Loopback only, with the internal credential:
// doc: the api's fitness sweep is the one caller.
export const Route = defineApi('/api/app-host/$app/harnesses/$id/cases/$name', {
  POST: async ({ request, params }) => {
    const gate = requireInternal(request)
    if (gate instanceof Response) return gate
    if (!slugOk(params.app)) return json({ error: 'bad slug' }, { status: 400 })
    const body = (await request.json().catch(() => null)) as { model?: unknown } | null
    const model = typeof body?.model === 'string' ? body.model.trim() : ''
    if (!model || model.length > MAX_MODEL) return json({ error: 'model is required' }, { status: 400 })
    let listing: Awaited<ReturnType<typeof listAppHarnesses>>
    try {
      listing = await listAppHarnesses()
    } catch (e) {
      return json({ error: `the enabled apps could not be read: ${e instanceof Error ? e.message : String(e)}` }, { status: 503 })
    }
    const harness = listing.harnesses.find((h) => h.slug === params.app && h.def.id === params.id)
    if (!harness) return json({ error: `no harness "${params.id}" on an enabled app "${params.app}"` }, { status: 404 })
    const replay = await replayAppCase(harness, params.name, model, { signal: request.signal })
    if (!replay) return json({ error: `harness "${params.id}" has no fixture "${params.name}"` }, { status: 404 })
    return json(replay)
  },
})
