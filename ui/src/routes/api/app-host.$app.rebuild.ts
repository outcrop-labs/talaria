import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireAdmin } from '@/server/api-guard'
import { reconcileApp } from '@/server/app-build/reconcile'
import { readCurrent } from '@/server/app-build/status'
import { slugOk } from '@/server/app-build/paths'
import { appSql } from '@/server/app-db'
import { isolateApp } from '@/server/app-isolate'
import { failHealth } from '@/server/app-health'

// POST /api/app-host/<slug>/rebuild — compile and load-probe one app now, and
// start its database (admin). Manage → Apps calls it before Enable and from
// Rebuild. It answers the build's outcome rather than failing the request: a
// broken app is an answer ({ ok: false, error }), not a server error.
//
// /api/app-host/ is the HOST's prefix for routes about an app. This route
// lived at /api/apps/<slug>/rebuild, inside the app's own namespace, and took
// that path from every app: an app serving `rebuild` got 405 on a GET and the
// host's rebuild on an admin's POST. Everything under /api/apps/<slug>/ is the
// app's now; the host's own app routes go here, a TS resident like the
// gateway (rust-proxy.ts STAY_TS).
export const Route = defineApi('/api/app-host/$app/rebuild', {
  POST: async ({ request, params }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    if (!slugOk(params.app)) return json({ error: 'bad slug' }, { status: 400 })
    const slug = params.app
    await reconcileApp(slug)
    let cur = readCurrent(slug)
    if (cur?.status === 'ready') {
      const db = await isolateApp(slug, 'database', () => appSql(slug).then(() => undefined))
      if (!db.ok) {
        await failHealth(slug, `database did not start: ${db.error}`)
        cur = readCurrent(slug)
      }
    }
    const ok = cur?.status === 'ready'
    return json({
      ok,
      slug,
      build: cur,
      ...(ok ? {} : { error: cur?.error ?? 'app failed to compile or load' }),
    })
  },
})
