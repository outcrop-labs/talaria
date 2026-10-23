import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { appServerModule } from '@/server/apps'
import { authorizeApp, canFor } from '@/server/app-access'
import { storeFor } from '@/server/app-store'
import type { AppServer } from '@/sdk/server'
import { isolateApp } from '@/server/app-isolate'

// The app-server gateway: /api/apps/<slug>/* dispatches into the app's own
// server.ts. The host does the trust work FIRST — the session and the grants
// as Rust resolves them, the app enabled, at least one of the app's surfaces
// granted (server/app-access.ts) — then hands the app a context that says
// which surfaces the caller holds, so the app can refuse its manage
// endpoints to a work-only caller. A throw in the app is isolated: the host
// stays up, the client gets a structured 500.
const dispatch = async ({ request, params }: { request: Request; params: { app: string; _splat?: string } }) => {
  const app = params.app
  const gate = await authorizeApp(request, app)
  if (!gate.ok) return gate.response
  const caller = gate.caller
  const ran = await isolateApp(app, 'server', async () => {
    const mod = (await appServerModule(app)) as { default?: AppServer } | null
    if (!mod?.default?.fetch) return json({ error: 'this app has no server' }, { status: 404 })
    return mod.default.fetch(request, {
      user: caller.user,
      app,
      path: params._splat ?? '',
      url: new URL(request.url),
      store: storeFor(app),
      access: { ...caller.access },
      perms: [...caller.perms],
      can: canFor(caller),
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
