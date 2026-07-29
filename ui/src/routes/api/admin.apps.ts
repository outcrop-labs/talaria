import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
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
import { deniedViews } from '@/server/users'
import { logAudit } from '@/server/audit'

// App administration. GET → installed apps (+ ?catalog=1 for the marketplace
// feed). Reads are open to anyone granted the /apps Manage view; mutations
// (enable/disable, install, uninstall, catalog source) stay admin-only —
// installing an app adds CODE to the deployment.
export const Route = createFileRoute('/api/admin/apps')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') {
          const denied = await deniedViews(user.id, user.role)
          if (denied.includes('/apps')) return json({ error: 'forbidden' }, { status: 403 })
        }
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
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const actor = user.email ?? user.name ?? 'admin'
        const parsed = z
          .union([
            z.object({ app: z.string().min(1), enabled: z.boolean() }),
            z.object({ installUrl: z.string().url(), slug: z.string().min(1).max(64).optional() }),
            z.object({ catalogUrl: z.string().url().nullable() }),
          ])
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          if ('enabled' in parsed.data) {
            await setAppEnabled(parsed.data.app, parsed.data.enabled)
            void logAudit({
              actor,
              action: parsed.data.enabled ? 'app.enable' : 'app.disable',
              targetType: 'app',
              targetId: parsed.data.app,
            })
            return json({ ok: true })
          }
          if ('installUrl' in parsed.data) {
            const r = await installAppFromGit(parsed.data.installUrl, parsed.data.slug)
            void logAudit({ actor, action: 'app.install', targetType: 'app', targetId: r.slug, targetLabel: parsed.data.installUrl })
            return json(r)
          }
          await setCatalogUrl(parsed.data.catalogUrl)
          void logAudit({ actor, action: 'app.catalog-url', targetType: 'app', targetLabel: parsed.data.catalogUrl ?? 'default' })
          return json({ ok: true, catalogUrl: await catalogUrl() })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
      DELETE: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = z
          .object({ app: z.string().min(1), wipeData: z.boolean().optional() })
          .safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          await uninstallApp(parsed.data.app)
          if (parsed.data.wipeData) await storeFor(parsed.data.app).wipe()
          void logAudit({
            actor: user.email ?? user.name ?? 'admin',
            action: 'app.uninstall',
            targetType: 'app',
            targetId: parsed.data.app,
          })
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
