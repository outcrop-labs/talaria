import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireInternal } from '@/server/internal-api'
import { slugOk } from '@/server/app-build/paths'
import { activateUpdate, release, stageUpdate } from '@/server/app-build/update'
import { stopAppDatabase } from '@/server/app-db'

// POST /api/app-host/<slug>/lifecycle — the host's steps of an app update and
// uninstall, called only by the api (talaria-apps `AppHost`) with its
// root-derived service credential; no session reaches it. Body: { step }.
//
//   stage     build and probe the new version the api fetched into
//             apps/.<slug>.next, beside the live app, which keeps serving
//   activate  after the api swapped the trees, switch the app to that build
//             and probe it live; a failure restores the previous build
//   release   let go of an update the api abandoned
//   removed   the api uninstalled the app: close its database pool
//
// Each answers { ok, build?, error? }. A failed step is an answer, not a
// server error, as Rebuild's is.
export const Route = defineApi('/api/app-host/$app/lifecycle', {
  POST: async ({ request, params }) => {
    const gate = requireInternal(request)
    if (gate) return gate
    if (!slugOk(params.app)) return json({ error: 'bad slug' }, { status: 400 })
    const slug = params.app
    const body = (await request.json().catch(() => null)) as { step?: unknown } | null
    switch (body?.step) {
      case 'stage':
        return json(await stageUpdate(slug))
      case 'activate':
        return json(await activateUpdate(slug))
      case 'release':
        release(slug)
        return json({ ok: true })
      case 'removed':
        release(slug)
        await stopAppDatabase(slug)
        return json({ ok: true })
      default:
        return json({ error: 'step must be stage, activate, release or removed' }, { status: 400 })
    }
  },
})
