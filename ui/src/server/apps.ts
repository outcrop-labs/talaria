// Talaria apps — self-contained codebases under apps/<slug>/ that compile
// INTO the deployment and render as native platform surfaces. This module is
// the server-side registry:
//
//   discovery    build-time glob of apps/*/talaria.json (what THIS build ships)
//   runtime dirs fs scan of the apps directory — catches apps installed after
//                the build (git clone) that need a dev-reload/rebuild to light up
//   enablement   admin-controlled set in app_settings; disabled apps have no
//                nav presence and their server routes 404
//   install      marketplace/git: shallow-clone a repo into apps/<slug>
//   catalog      the marketplace index — a JSON feed of community + official
//                apps, source URL configurable (defaults to Outcrop's registry)
import { readdir, rm, stat } from 'node:fs/promises'
import { resolve, basename } from 'node:path'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { getSetting, setSetting } from './audit'
import { appHasMcp } from './app-mcp'
import { safeFetch } from './safe-fetch'

const execFileP = promisify(execFile)

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

export function discoveredApps(): AppManifest[] {
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

export async function setAppEnabled(slug: string, enabled: boolean): Promise<void> {
  const cur = new Set(await enabledAppSlugs())
  if (enabled) {
    if (!discoveredApps().some((a) => a.slug === slug)) throw new Error(`no app "${slug}" in this build`)
    cur.add(slug)
  } else {
    cur.delete(slug)
  }
  await setSetting(ENABLED_KEY, [...cur].sort())
  // Apps that publish MCP tools follow their enablement into the registry
  // (rows appear/disappear; carriers get rolled on removal).
  const { syncAppMcpServers } = await import('./mcp-registry')
  await syncAppMcpServers().catch(() => {})
}

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

// ── Runtime install (marketplace / git) ────────────────────────────────────

/** Where app codebases live on disk (git-clone target). In dev this is the
 *  repo's apps/ dir; deployments can point elsewhere via TALARIA_APPS_DIR. */
export const appsDir = (): string => process.env.TALARIA_APPS_DIR ?? resolve(process.cwd(), '../apps')

/** Apps present on disk but NOT in this build — installed after the last
 *  compile; the dev server picks them up on reload, prod needs a rebuild. */
export async function pendingApps(): Promise<string[]> {
  const built = new Set(discoveredApps().map((a) => a.slug))
  try {
    const entries = await readdir(appsDir(), { withFileTypes: true })
    const dirs = entries.filter((e) => e.isDirectory() && SLUG_RE.test(e.name) && !built.has(e.name))
    const withManifest: string[] = []
    for (const d of dirs) {
      try {
        await stat(resolve(appsDir(), d.name, 'talaria.json'))
        withManifest.push(d.name)
      } catch {
        /* not an app dir */
      }
    }
    return withManifest
  } catch {
    return []
  }
}

/** Install an app by shallow-cloning its git repo into apps/<slug>. The code
 *  becomes part of the NEXT build (dev picks it up live) — and, like anything
 *  compiled into the deployment, runs fully trusted. The UI says so. */
export async function installAppFromGit(url: string, slugOverride?: string): Promise<{ slug: string; pendingBuild: boolean }> {
  const u = url.trim()
  if (!/^https:\/\/[^\s]+$/.test(u)) throw new Error('install URL must be https://')
  const derived = (slugOverride ?? basename(u).replace(/\.git$/, '')).toLowerCase().replace(/^talaria-app-/, '')
  if (!SLUG_RE.test(derived)) throw new Error(`"${derived}" is not a usable app slug (lowercase letters, digits, dashes)`)
  const target = resolve(appsDir(), derived)
  if (!target.startsWith(appsDir())) throw new Error('bad target path')
  try {
    await stat(target)
    throw new Error(`apps/${derived} already exists`)
  } catch (e) {
    if ((e as NodeJS.ErrnoException).code !== 'ENOENT') throw e
  }
  await execFileP('git', ['clone', '--depth', '1', u, target], { timeout: 60_000 })
  try {
    await stat(resolve(target, 'talaria.json'))
  } catch {
    await rm(target, { recursive: true, force: true })
    throw new Error('that repository is not a Talaria app (no talaria.json at its root)')
  }
  const installed = await getSetting<Record<string, { source: string; installedAt: string }>>('apps_installed', {})
  await setSetting('apps_installed', { ...installed, [derived]: { source: u, installedAt: new Date().toISOString() } })
  const pendingBuild = !discoveredApps().some((a) => a.slug === derived)
  return { slug: derived, pendingBuild }
}

/** Remove an app's codebase + enablement. Data in the app store is wiped by
 *  the caller (it owns the confirm). Only touches dirs inside appsDir. */
export async function uninstallApp(slug: string): Promise<void> {
  if (!SLUG_RE.test(slug)) throw new Error('bad slug')
  await setAppEnabled(slug, false)
  const target = resolve(appsDir(), slug)
  if (!target.startsWith(appsDir())) throw new Error('bad target path')
  await rm(target, { recursive: true, force: true })
  const installed = await getSetting<Record<string, unknown>>('apps_installed', {})
  const { [slug]: _gone, ...rest } = installed
  await setSetting('apps_installed', rest)
}

export const installedSources = () => getSetting<Record<string, { source: string; installedAt: string }>>('apps_installed', {})

// ── Marketplace catalog ────────────────────────────────────────────────────

const CATALOG_URL_KEY = 'apps_catalog_url'
const DEFAULT_CATALOG = 'https://raw.githubusercontent.com/outcrop-labs/talaria-apps/main/index.json'

export const catalogUrl = async (): Promise<string> => (await getSetting<string>(CATALOG_URL_KEY, DEFAULT_CATALOG)) || DEFAULT_CATALOG
export const setCatalogUrl = (url: string | null) => setSetting(CATALOG_URL_KEY, url?.trim() || DEFAULT_CATALOG)

export interface CatalogApp {
  slug: string
  name: string
  icon: string
  description: string
  repo: string
  author: string
  official: boolean
  version?: string
}

/** The marketplace feed. Unreachable/invalid → empty list with the error, so
 *  the Discover tab can say why it's blank instead of pretending it's empty. */
export async function fetchCatalog(): Promise<{ apps: CatalogApp[]; error?: string }> {
  const url = await catalogUrl()
  try {
    // Admin-settable feed URL — through the SSRF guard, so a mistyped (or
    // malicious) catalog can't turn the Discover tab into an internal scanner.
    const r = await safeFetch(url, { timeoutMs: 10_000, maxBytes: 2 * 1024 * 1024, headers: { accept: 'application/json' } })
    if (!r.ok) return { apps: [], error: `catalog fetch failed (${r.status})` }
    const j = (await r.json()) as { apps?: Array<Partial<CatalogApp>> }
    const apps = (j.apps ?? [])
      .filter((a) => a.slug && SLUG_RE.test(a.slug) && a.name && a.repo && /^https:\/\//.test(a.repo))
      .map((a) => ({
        slug: a.slug!,
        name: String(a.name),
        icon: String(a.icon ?? '⬡'),
        description: String(a.description ?? ''),
        repo: String(a.repo),
        author: String(a.author ?? 'community'),
        official: a.official === true,
        ...(a.version ? { version: String(a.version) } : {}),
      }))
    return { apps }
  } catch (e) {
    return { apps: [], error: (e as Error).message }
  }
}
