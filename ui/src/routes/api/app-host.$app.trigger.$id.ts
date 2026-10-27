import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireInternal } from '@/server/internal-guard'
import { slugOk } from '@/server/app-build/paths'
import { parseTriggerBody, runAppTrigger } from '@/server/app-trigger'

// doc: POST /api/app-host/<slug>/trigger/<id> — run one of an enabled app's
// doc: triggers: the handler its server names for that id
// doc: (`defineAppServer({ triggers: { <id>: (ctx) => … } })`), as the app,
// doc: with no user. Body: `{ runId, kind: "cron" | "event", dueAt?, event?,
// doc: payload, deadlineMs }` — the schedule's minute, or the platform event
// doc: and its ids. Answers the run's outcome, `{ ok: true, detail? }` (the
// doc: string the handler returned) or `{ ok: false, error }` (what it threw,
// doc: or that the deadline cut it); 404 when the app's server has no handler
// doc: for the id, 409 when the app is not enabled. Loopback only, with the
// doc: internal credential: the api's trigger scheduler is the one caller, and
// doc: it records every run for Manage → Apps.
export const Route = defineApi('/api/app-host/$app/trigger/$id', {
  POST: async ({ request, params }) => {
    const gate = requireInternal(request)
    if (gate instanceof Response) return gate
    if (!slugOk(params.app) || !slugOk(params.id)) return json({ error: 'bad slug or trigger id' }, { status: 400 })
    const parsed = parseTriggerBody(await request.json().catch(() => null))
    if (!parsed.ok) return json({ error: parsed.error }, { status: 400 })
    const outcome = await runAppTrigger(params.app, params.id, parsed.body, request.signal)
    return json(outcome.body, { status: outcome.status })
  },
})
