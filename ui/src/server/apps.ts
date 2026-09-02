// Talaria apps — self-contained codebases under apps/<slug>/ that compile
// INTO the deployment and render as native platform surfaces. This module is
// the serving registry: what this build ships (build-time manifest glob),
// which of those are enabled (the admin-controlled set in app_settings), and
// the loaders the app-server gateway and nav/views need. The LIFECYCLE —
// enable/disable, git install, uninstall, the marketplace catalog — is the
// Rust twin's (api/src/apps.rs, behind /api/admin/apps).
import { basename } from 'node:path'
import { getSetting } from './audit'
import { appHasMcp } from './app-mcp'

export interface AppManifest {
  slug: string
  name: string
  icon: string
  description: string
  version: string
  /** Surface labels; presence of a key = the app provides that surface. */
  surfaces: { work?: string; manage?: string; settings?: string }
  /** The app publishes MCP tools for agents (apps/<slug>/mcp.ts). */
  mcp: boolean
}

interface RawManifest {
  name?: string
  icon?: string
  description?: string
  version?: string
  surfaces?: { work?: string; manage?: string; settings?: string }
}

// Build-time discovery: every app the current build compiled in.
const MANIFESTS = import.meta.glob('../../../apps/*/talaria.json', { eager: true, import: 'default' }) as Record<string, RawManifest>
// Lazy loaders for each app's optional server module.
const SERVER_MODS = import.meta.glob('../../../apps/*/server.ts') as Record<string, () => Promise<unknown>>

const slugOf = (path: string): string => /apps\/([^/]+)\//.exec(path)?.[1] ?? basename(path)

const SLUG_RE = /^[a-z0-9][a-z0-9-]{0,63}$/

function discoveredApps(): AppManifest[] {
  return Object.entries(MANIFESTS)
    .map(([path, m]) => {
      const slug = slugOf(path)
      if (!SLUG_RE.test(slug) || !m?.name) return null
      return {
        slug,
        name: String(m.name),
        icon: String(m.icon ?? '⬡'),
        description: String(m.description ?? ''),
        version: String(m.version ?? '0.0.0'),
        surfaces: {
          ...(m.surfaces?.work ? { work: String(m.surfaces.work) } : {}),
          ...(m.surfaces?.manage ? { manage: String(m.surfaces.manage) } : {}),
          ...(m.surfaces?.settings ? { settings: String(m.surfaces.settings) } : {}),
        },
        mcp: appHasMcp(slug),
      } satisfies AppManifest
    })
    .filter((a): a is AppManifest => a !== null)
    .sort((a, b) => a.name.localeCompare(b.name))
}

const ENABLED_KEY = 'apps_enabled'

export const enabledAppSlugs = () => getSetting<string[]>(ENABLED_KEY, [])

export async function enabledApps(): Promise<AppManifest[]> {
  const on = new Set(await enabledAppSlugs())
  return discoveredApps().filter((a) => on.has(a.slug))
}

/** Loader for an app's server module, or null when the app ships none. */
export function appServerModule(slug: string): (() => Promise<unknown>) | null {
  const entry = Object.entries(SERVER_MODS).find(([p]) => slugOf(p) === slug)
  return entry?.[1] ?? null
}

/** ALL app view routes of ENABLED apps (work + manage surfaces). Apps are
 *  explicit-grant: members get NO app access by default — an admin allows
 *  each view per person, same flow as core Manage views. */
export async function appViewRoutes(): Promise<string[]> {
  const apps = await enabledApps()
  return [
    ...apps.filter((a) => a.surfaces.work).map((a) => `/x/${a.slug}`),
    ...apps.filter((a) => a.surfaces.manage).map((a) => `/x/${a.slug}/manage`),
  ]
}
