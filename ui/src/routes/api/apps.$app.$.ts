import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { appServerModule, enabledAppSlugs } from '@/server/apps'
import { storeFor } from '@/server/app-store'
import { deniedViews } from '@/server/users'
import type { AppServer } from '@/sdk/server'

// The app-server gateway: /api/apps/<slug>/* dispatches into the app's own
// server.ts. The host does the trust work FIRST — session, app enabled, user
// not view-denied — then hands the app a context (user, sub-path, namespaced
// store). Apps never see raw cookies or other apps' data.
const dispatch = async ({ request, params }: { request: Request; params: { app: string; _splat?: string } }) => {
  const user = await getSessionUser(request)
  if (!user) return json({ error: 'unauthorized' }, { status: 401 })
  const app = params.app
  if (!(await enabledAppSlugs()).includes(app)) return json({ error: 'no such app' }, { status: 404 })
  const denied = await deniedViews(user.id, user.role)
  if (denied.some((v) => v === `/x/${app}` || `/x/${app}`.startsWith(v + '/'))) {
    return json({ error: 'forbidden' }, { status: 403 })
  }
  const loader = appServerModule(app)
  if (!loader) return json({ error: 'this app has no server' }, { status: 404 })
  const mod = (await loader()) as { default?: AppServer }
  if (!mod.default?.fetch) return json({ error: 'this app has no server' }, { status: 404 })
  try {
    return await mod.default.fetch(request, {
      user,
      app,
      path: params._splat ?? '',
      url: new URL(request.url),
      store: storeFor(app),
    })
  } catch (e) {
    // App exceptions log server-side; clients get a generic 500 — internal
    // messages can embed paths/config and reach any signed-in member here.
    console.error(`[apps:${app}]`, e)
    return json({ error: 'app error' }, { status: 500 })
  }
}

export const Route = createFileRoute('/api/apps/$app/$')({
  server: {
    handlers: {
      GET: dispatch,
      POST: dispatch,
      PUT: dispatch,
      PATCH: dispatch,
      DELETE: dispatch,
    },
  },
})
