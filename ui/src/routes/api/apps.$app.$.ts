import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { requireUser } from '@/server/api-guard'
import { appServerModule, enabledAppSlugs } from '@/server/apps'
import { storeFor } from '@/server/app-store'
import { deniedViews } from '@/server/users'
import type { AppServer } from '@/sdk/server'
import { isolateApp } from '@/server/app-isolate'

// The app-server gateway: /api/apps/<slug>/* dispatches into the app's own
// server.ts. The host does the trust work FIRST — session, app enabled, user
// not view-denied — then hands the app a context. A throw in the app is
// isolated: the host stays up, the client gets a structured 500.
const dispatch = async ({ request, params }: { request: Request; params: { app: string; _splat?: string } }) => {
  const gate = await requireUser(request)
  if (gate instanceof Response) return gate
  const user = gate
  const app = params.app
  if (!(await enabledAppSlugs()).includes(app)) return json({ error: 'no such app' }, { status: 404 })
  const denied = await deniedViews(user.id, user.role)
  if (denied.some((v) => v === `/x/${app}` || `/x/${app}`.startsWith(v + '/'))) {
    return json({ error: 'forbidden' }, { status: 403 })
  }
  const ran = await isolateApp(app, 'server', async () => {
    const mod = (await appServerModule(app)) as { default?: AppServer } | null
    if (!mod?.default?.fetch) return json({ error: 'this app has no server' }, { status: 404 })
    return mod.default.fetch(request, {
      user,
      app,
      path: params._splat ?? '',
      url: new URL(request.url),
      store: storeFor(app),
    })
  })
  if (!ran.ok) return json({ error: 'app crashed', detail: ran.error }, { status: 500 })
  return ran.value
}

export const Route = defineApi('/api/apps/$app/$', {
  GET: dispatch,
  POST: dispatch,
  PUT: dispatch,
  PATCH: dispatch,
  DELETE: dispatch,
})
