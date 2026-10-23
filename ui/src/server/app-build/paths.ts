// Where app source and app build artifacts live. Env overrides match the
// Rust twins (api/src/users.rs). process.cwd() is ui/ in both vite dev and
// server-entry — never import.meta.url, which this file's bundled form
// would resolve from dist/server/.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'

const SLUG_RE = /^[a-z0-9][a-z0-9-]{0,63}$/

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

// A manifest that is not UTF-8 is not an app. `readFileSync(…, 'utf8')`
// decoded bad bytes to U+FFFD and admitted the app, while Rust's
// `discovered_apps_in` skips it, so the two hosts disagreed about which apps
// exist. Fatal, and the BOM kept, so a byte-order mark fails JSON.parse the
// way it fails serde_json (api/tests/it/fixtures/app-grants.json pins both).
const MANIFEST_DECODER = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })

export function listInstalledApps(): InstalledApp[] {
  const dir = appsDir()
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return []
  const out: InstalledApp[] = []
  for (const name of readdirSync(dir)) {
    if (!slugOk(name) || !statSync(join(dir, name)).isDirectory()) continue
    const raw = (() => {
      try {
        return JSON.parse(MANIFEST_DECODER.decode(readFileSync(join(dir, name, 'talaria.json')))) as {
          name?: unknown
          icon?: unknown
          description?: unknown
          version?: unknown
          surfaces?: { work?: unknown; manage?: unknown; settings?: unknown }
        }
      } catch {
        return null
      }
    })()
    if (!raw || typeof raw.name !== 'string' || !raw.name) continue
    const surface = (key: 'work' | 'manage' | 'settings') => {
      const v = raw.surfaces?.[key]
      return typeof v === 'string' && v ? v : undefined
    }
    out.push({
      slug: name,
      name: raw.name,
      icon: typeof raw.icon === 'string' && raw.icon ? raw.icon : '⬡',
      description: typeof raw.description === 'string' ? raw.description : '',
      version: typeof raw.version === 'string' && raw.version ? raw.version : '0.0.0',
      surfaces: {
        ...(surface('work') ? { work: surface('work') } : {}),
        ...(surface('manage') ? { manage: surface('manage') } : {}),
        ...(surface('settings') ? { settings: surface('settings') } : {}),
      },
      hasServer: existsSync(join(dir, name, 'server.ts')),
      hasMcp: existsSync(join(dir, name, 'mcp.ts')),
    })
  }
  return out.sort((a, b) => a.name.localeCompare(b.name))
}

/** The surfaces an app's talaria.json declares (work, manage, settings) —
 *  empty for a server- or MCP-only app, or one whose manifest is not an app.
 *  An app that declares none ships no client, and app.ts is optional. */
export function declaredSurfaces(slug: string): Array<'work' | 'manage' | 'settings'> {
  const app = listInstalledApps().find((a) => a.slug === slug)
  if (!app) return []
  return (['work', 'manage', 'settings'] as const).filter((k) => app.surfaces[k] !== undefined)
}

/** Why an app with no app.ts cannot be built or enabled, or null when it
 *  can: it declares no surface, and it has a server.ts or an mcp.ts to run.
 *  One rule for the build (build.ts) and the load probe (app-health.ts). */
export function clientlessError(declared: readonly string[], hasServerSide: boolean): string | null {
  if (declared.length > 0) return `talaria.json declares surfaces (${declared.join(', ')}), so app.ts is required`
  if (!hasServerSide) return 'an app needs at least one of app.ts, server.ts or mcp.ts'
  return null
}
