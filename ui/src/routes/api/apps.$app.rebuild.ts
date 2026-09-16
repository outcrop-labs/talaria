import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireAdmin } from '@/server/api-guard'
import { reconcileApp } from '@/server/app-build/reconcile'
import { readCurrent } from '@/server/app-build/status'
import { slugOk } from '@/server/app-build/paths'
import { appSql } from '@/server/app-db'
import { isolateApp } from '@/server/app-isolate'
import { failHealth } from '@/server/app-health'

export const Route = defineApi('/api/apps/$app/rebuild', {
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
