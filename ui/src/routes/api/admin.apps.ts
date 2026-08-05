import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin, requireView } from '@/server/api-guard'
import {
  catalogUrl,
  discoveredApps,
  enabledAppSlugs,
  fetchCatalog,
  installAppFromGit,
  installedSources,
  pendingApps,
  setAppEnabled,
  setCatalogUrl,
  uninstallApp,
} from '@/server/apps'
import { storeFor } from '@/server/app-store'
import { logAudit } from '@/server/audit'

// App administration. GET → installed apps (+ ?catalog=1 for the marketplace
// feed). Reads are open to anyone granted the /apps Manage view; mutations
// (enable/disable, install, uninstall, catalog source) stay admin-only —
// installing an app adds CODE to the deployment.
export const Route = defineApi('/api/admin/apps', {
  GET: async ({ request }) => {
    const gate = await requireView(request, '/apps')
    if (gate instanceof Response) return gate
    const [enabled, pending, sources] = await Promise.all([enabledAppSlugs(), pendingApps(), installedSources()])
    const apps = discoveredApps().map((a) => ({
      ...a,
      enabled: enabled.includes(a.slug),
      source: sources[a.slug]?.source ?? null,
    }))
    const wantCatalog = new URL(request.url).searchParams.get('catalog') === '1'
    const catalog = wantCatalog ? await fetchCatalog() : null
    return json({ apps, pending, catalog, catalogUrl: await catalogUrl() })
  },
  // PUT → config writes (enable/disable, catalog source); POST → install
  // (the action that pulls code into the deployment).
  PUT: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const actor = actorOf(user)
    const body = await parseBody(
      request,
      z.union([
        z.object({ app: z.string().min(1), enabled: z.boolean() }),
        z.object({ catalogUrl: z.string().url().nullable() }),
      ]),
    )
    if (body instanceof Response) return body
    try {
      if ('enabled' in body) {
        await setAppEnabled(body.app, body.enabled)
        void logAudit({
          actor,
          action: body.enabled ? 'app.enable' : 'app.disable',
          targetType: 'app',
          targetId: body.app,
        })
        return json({ ok: true })
      }
      await setCatalogUrl(body.catalogUrl)
      void logAudit({ actor, action: 'app.catalog-url', targetType: 'app', targetLabel: body.catalogUrl ?? 'default' })
      return json({ ok: true, catalogUrl: await catalogUrl() })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(
      request,
      z.object({ installUrl: z.string().url(), slug: z.string().min(1).max(64).optional() }),
    )
    if (body instanceof Response) return body
    try {
      const r = await installAppFromGit(body.installUrl, body.slug)
      void logAudit({ actor: actorOf(user), action: 'app.install', targetType: 'app', targetId: r.slug, targetLabel: body.installUrl })
      return json(r)
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
  DELETE: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, z.object({ app: z.string().min(1), wipeData: z.boolean().optional() }))
    if (body instanceof Response) return body
    try {
      await uninstallApp(body.app)
      if (body.wipeData) await storeFor(body.app).wipe()
      void logAudit({
        actor: actorOf(user),
        action: 'app.uninstall',
        targetType: 'app',
        targetId: body.app,
      })
      return json({ ok: true })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
})
