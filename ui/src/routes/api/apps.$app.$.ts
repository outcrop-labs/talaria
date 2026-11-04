import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { appServerModule } from '@/server/apps'
import { authorizeApp, canFor } from '@/server/app-access'
import { appOrigin, appPathSegments, appSecretReader, appServices, platformApi } from '@/server/app-context'
import type { AppServer } from '@/sdk/server'
import { isolateApp } from '@/server/app-isolate'

// The app-server gateway: /api/apps/<slug>/* dispatches into the app's own
// server.ts. The host does the trust work FIRST — the session and the grants
// as Rust resolves them, the app enabled, at least one of the app's surfaces
// granted (server/app-access.ts) — then hands the app a context that says
// which surfaces the caller holds, so the app can refuse its manage
// endpoints to a work-only caller. A throw in the app is isolated: the host
// stays up, the client gets a structured 500.
//
// EVERY PATH UNDER /api/apps/<slug>/ IS THE APP'S. The host's own routes about
// an app (rebuild) live under /api/app-host/<slug>/, where they cannot take a
// path the app serves.
const dispatch = async ({ request, params }: { request: Request; params: { app: string; _splat?: string } }) => {
  const app = params.app
  const gate = await authorizeApp(request, app)
  if (!gate.ok) return gate.response
  const caller = gate.caller
  // The sub-path from the RAW url, decoded once per segment here — not from
  // `_splat`, where an encoded slash would be indistinguishable from a
  // separator.
  const path = appPathSegments(new URL(request.url).pathname)
  if (!path.ok) return json({ error: path.error }, { status: 400 })
  const segments = path.segments
  const api = platformApi(request)
  // Everything bound to this caller — the store, the agents they may drive,
  // the harness runner — from the one factory every context is built
  // through. An agent pin is checked against the agents the api lets THEM
  // use (`usable_agent_gate`, asked with their own cookie through the same
  // client the app gets as `ctx.api`).
  const services = appServices(app, { kind: 'request', user: { id: caller.user.id, role: caller.user.role } }, request.signal, {
    usableAgents: async () => ((await api<{ agents?: Array<{ id: string }> }>('/api/agents'))?.agents ?? []).map((a) => a.id),
  })
  const ran = await isolateApp(app, 'server', async () => {
    const mod = (await appServerModule(app)) as { default?: AppServer } | null
    if (!mod?.default?.fetch) return json({ error: 'this app has no server' }, { status: 404 })
    try {
      return await mod.default.fetch(request, {
        user: caller.user,
        app,
        path: segments.join('/'),
        segments: [...segments],
        url: new URL(request.url),
        origin: appOrigin(request),
        store: services.store,
        access: { ...caller.access },
        perms: [...caller.perms],
        can: canFor(caller),
        api,
        secret: appSecretReader(app, caller.user.id),
        signal: request.signal,
        agents: services.agents,
        harness: services.harness,
      })
    } catch (e) {
      // The caller left and the app's work was cut with it (ctx.signal):
      // that is not a crash, and nobody is waiting for the answer.
      if (request.signal.aborted) return new Response(null, { status: 499 })
      throw e
    }
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
