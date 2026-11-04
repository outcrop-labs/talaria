import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireInternal } from '@/server/internal-guard'
import { listAppHarnesses, refreshAppHarnessReports, wireOf } from '@/server/app-harnesses'

// doc: GET /api/app-host/harnesses — every enabled app's activity harnesses
// doc: (`apps/<slug>/harnesses/*.ts`), as data: id, label, job, requirements,
// doc: floor, guard rules, output kind, fixtures with their names and bands,
// doc: and the source `app:<slug>`. Never code: the api registers them beside
// doc: its builtins for the model-fitness matrix and replays their fixtures
// doc: through `POST /api/app-host/<slug>/harnesses/<id>/cases/<name>`.
// doc: `problems` names every harness file that was skipped and why.
// doc: POST rewrites every enabled app's harness report (the "N harnesses"
// doc: and "N skipped" lines in Manage → Apps) from one listing: the api
// doc: calls it after an app is enabled, disabled or uninstalled, since which
// doc: app holds a contested id moves with the enabled set. Loopback only,
// doc: with the internal credential: the api is the one caller.
export const Route = defineApi('/api/app-host/harnesses', {
  GET: async ({ request }) => {
    const gate = requireInternal(request)
    if (gate instanceof Response) return gate
    try {
      const listing = await listAppHarnesses()
      return json({ harnesses: listing.harnesses.map(wireOf), problems: listing.problems })
    } catch (e) {
      // The enabled set could not be read (a malformed `apps_enabled`): say
      // so rather than answer an empty list the api would read as "no apps".
      return json({ error: `the enabled apps could not be read: ${e instanceof Error ? e.message : String(e)}` }, { status: 503 })
    }
  },
  POST: async ({ request }) => {
    const gate = requireInternal(request)
    if (gate instanceof Response) return gate
    try {
      const reports = await refreshAppHarnessReports()
      return json({ ok: true, apps: [...reports.keys()] })
    } catch (e) {
      return json({ error: `the enabled apps could not be read: ${e instanceof Error ? e.message : String(e)}` }, { status: 503 })
    }
  },
})
