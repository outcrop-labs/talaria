// Where app source and app build artifacts live. Env overrides match the
// Rust twins (api/crates/talaria-users). process.cwd() is ui/ in both vite dev and
// server-entry — never import.meta.url, which this file's bundled form
// would resolve from dist/server/.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { SLUG_RE, parseManifest, type ManifestResult, type SurfaceKey } from './manifest'

export const slugOk = (s: string): boolean => SLUG_RE.test(s)

export function uiRoot(): string {
  return process.cwd()
}

export function appsDir(): string {
  return process.env.TALARIA_APPS_DIR || join(uiRoot(), '..', 'apps')
}

export function appBuildsDir(): string {
  return process.env.TALARIA_APP_BUILDS_DIR || join(dirname(appsDir()), 'app-builds')
}

/** One app's data directory: its database files and the records the host
 *  keeps about it (the MCP catalog). Never served over HTTP, unlike
 *  `appBuildsDir()`. Rust twin: talaria-users `app_data_dir`. */
export function appDataDir(slug: string): string {
  const root = process.env.TALARIA_APP_DATA_DIR || join(appsDir(), '..', 'app-data')
  return join(root, slug)
}

export function appSrc(slug: string, file: string): string {
  return join(appsDir(), slug, file)
}

/** The live tree of an installed app: `apps/<slug>`. */
export function appDir(slug: string): string {
  return join(appsDir(), slug)
}

/** An update's staging tree, `apps/.<slug>.next`: the new version, fetched by
 *  the api and built and probed here before it is swapped in. Dot-named, so
 *  neither discovery nor the watcher ever takes it for an app. */
export function appNextDir(slug: string): string {
  return join(appsDir(), `.${slug}.next`)
}

/** A build key is the hex digest app-build/key.ts writes (`sourceKey`). */
const BUILD_KEY_RE = /^[0-9a-f]{1,64}$/

export const buildKeyOk = (s: string): boolean => BUILD_KEY_RE.test(s)

export function appBuildKeyDir(slug: string, key: string): string {
  return join(appBuildsDir(), slug, key)
}

export interface InstalledApp {
  slug: string
  name: string
  icon: string
  description: string
  version: string
  surfaces: { work?: string; manage?: string; settings?: string }
  hasServer: boolean
  hasMcp: boolean
}

/** The manifest at the root of `dir`, read and held to the rules
 *  (manifest.ts — the TS reading of Rust's talaria-app-manifest). Null when
 *  there is no talaria.json at all: not an app. */
export function readManifestAt(dir: string): ManifestResult | null {
  let bytes: Uint8Array
  try {
    bytes = readFileSync(join(dir, 'talaria.json'))
  } catch {
    return null
  }
  return parseManifest(bytes)
}

/** The apps under the apps dir, sorted by name. An app is a slug-named
 *  directory whose talaria.json passes the manifest rules — the judgement
 *  Rust's discovery makes (talaria-users `discovered_apps_in`), pinned by
 *  api/tests/it/fixtures/app-manifest.json. One whose manifest fails, or that
 *  declares a slug other than its directory's, is not an app here: the api
 *  lists it as broken, and nothing builds or mounts it. */
export function listInstalledApps(): InstalledApp[] {
  const dir = appsDir()
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return []
  const out: InstalledApp[] = []
  for (const name of readdirSync(dir)) {
    if (!slugOk(name) || !statSync(join(dir, name)).isDirectory()) continue
    const read = readManifestAt(join(dir, name))
    if (!read?.ok) continue
    const m = read.manifest
    if (m.slug !== undefined && m.slug !== name) continue
    out.push({
      slug: name,
      name: m.name,
      icon: m.icon,
      description: m.description,
      version: m.version,
      surfaces: { ...m.surfaces },
      hasServer: existsSync(join(dir, name, 'server.ts')),
      hasMcp: existsSync(join(dir, name, 'mcp.ts')),
    })
  }
  return out.sort((a, b) => a.name.localeCompare(b.name))
}

/** Why the tree at `dir` is not an app it could be installed as `slug`, or
 *  null when it is: the manifest rules, and a declared slug that must match.
 *  The build refuses on the same sentence (build.ts). */
export function manifestError(dir: string, slug: string): string | null {
  const read = readManifestAt(dir)
  if (!read) return `no app "${slug}" (no talaria.json)`
  if (!read.ok) return read.error
  if (read.manifest.slug !== undefined && read.manifest.slug !== slug) {
    return `talaria.json declares the slug "${read.manifest.slug}", but the app is installed as "${slug}"`
  }
  return null
}

/** The surfaces the manifest at `dir` declares — empty when it declares none
 *  or is not an app. */
export function declaredSurfacesAt(dir: string): SurfaceKey[] {
  const read = readManifestAt(dir)
  if (!read?.ok) return []
  return (['work', 'manage', 'settings'] as const).filter((k) => read.manifest.surfaces[k] !== undefined)
}

/** The surfaces an app's talaria.json declares (work, manage, settings) —
 *  empty for a server- or MCP-only app, or one whose manifest is not an app.
 *  An app that declares none ships no client, and app.ts is optional. */
export function declaredSurfaces(slug: string): Array<'work' | 'manage' | 'settings'> {
  if (!slugOk(slug)) return []
  return declaredSurfacesAt(appDir(slug))
}

/** Why an app with no app.ts cannot be built or enabled, or null when it
 *  can: it declares no surface, and it has a server.ts or an mcp.ts to run.
 *  One rule for the build (build.ts) and the load probe (app-health.ts). */
export function clientlessError(declared: readonly string[], hasServerSide: boolean): string | null {
  if (declared.length > 0) return `talaria.json declares surfaces (${declared.join(', ')}), so app.ts is required`
  if (!hasServerSide) return 'an app needs at least one of app.ts, server.ts or mcp.ts'
  return null
}

/** A file under `apps/<slug>/harnesses/` that holds one harness: a `.ts`
 *  module name, nothing nested. */
export const HARNESS_FILE = /^[A-Za-z0-9][A-Za-z0-9._-]*\.ts$/

/** `harnesses/*.ts` in an app's source, sorted — one level, and never a test
 *  or a declaration file. The build (build.ts) compiles exactly this list and
 *  the dev loader (app-load.ts) loads exactly this list. */
export function harnessSourceFiles(dir: string): string[] {
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return []
  return readdirSync(dir, { withFileTypes: true })
    .filter((d) => d.isFile() && HARNESS_FILE.test(d.name) && !/\.(test|spec|d)\.ts$/.test(d.name))
    .map((d) => d.name)
    .sort()
}
