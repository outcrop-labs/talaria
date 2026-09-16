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

export function appSrc(slug: string, file: string): string {
  return join(appsDir(), slug, file)
}

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

export function listInstalledApps(): InstalledApp[] {
  const dir = appsDir()
  if (!existsSync(dir) || !statSync(dir).isDirectory()) return []
  const out: InstalledApp[] = []
  for (const name of readdirSync(dir)) {
    if (!slugOk(name) || !statSync(join(dir, name)).isDirectory()) continue
    const raw = (() => {
      try {
        return JSON.parse(readFileSync(join(dir, name, 'talaria.json'), 'utf8')) as {
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
