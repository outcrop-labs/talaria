// Talaria apps — the serving registry. Discovery is a disk scan of
// TALARIA_APPS_DIR (the host build no longer globs apps/). Enablement is
// the admin-controlled set in app_settings. Loaders pull independently
// built artifacts (or Vite HMR in dev). The LIFECYCLE — enable/disable,
// git install, uninstall, the marketplace catalog — is the Rust twin's
// (api/src/apps.rs, behind /api/admin/apps).
import { getSetting } from './audit'
import { listInstalledApps } from './app-build/paths'
import { appHasMcp, loadAppServer } from './app-load'

export interface AppManifest {
  slug: string
  name: string
  icon: string
  description: string
  version: string
  surfaces: { work?: string; manage?: string; settings?: string }
  mcp: boolean
}

const ENABLED_KEY = 'apps_enabled'

export const enabledAppSlugs = () => getSetting<string[]>(ENABLED_KEY, [])

export async function enabledApps(): Promise<AppManifest[]> {
  const on = new Set(await enabledAppSlugs())
  return listInstalledApps()
    .filter((a) => on.has(a.slug))
    .map((a) => ({
      slug: a.slug,
      name: a.name,
      icon: a.icon,
      description: a.description,
      version: a.version,
      surfaces: a.surfaces,
      mcp: a.hasMcp || appHasMcp(a.slug),
    }))
}

export async function appServerModule(slug: string): Promise<unknown> {
  return loadAppServer(slug)
}

/** ALL app view routes of ENABLED apps (work + manage surfaces). */
export async function appViewRoutes(): Promise<string[]> {
  const apps = await enabledApps()
  return [
    ...apps.filter((a) => a.surfaces.work).map((a) => `/x/${a.slug}`),
    ...apps.filter((a) => a.surfaces.manage).map((a) => `/x/${a.slug}/manage`),
  ]
}
