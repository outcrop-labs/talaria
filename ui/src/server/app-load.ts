// Load an app's server/mcp/client module and its harness files. Dev: Vite
// SSR graph (HMR). Prod: the independently-built artifact. Never a host glob
// — the host build does not see apps/.
import { existsSync } from 'node:fs'
import { pathToFileURL } from 'node:url'
import { appSrc, HARNESS_FILE, harnessSourceFiles, slugOk } from './app-build/paths'
import { readCurrent } from './app-build/status'
import { appBuildKeyDir } from './app-build/paths'

let ssrLoad: ((abs: string) => Promise<unknown>) | null = null

export function setSsrLoader(fn: ((abs: string) => Promise<unknown>) | null): void {
  ssrLoad = fn
}

async function importFile(abs: string): Promise<unknown> {
  if (!existsSync(abs)) return null
  if (ssrLoad) return ssrLoad(abs)
  return import(pathToFileURL(abs).href)
}

function builtFile(slug: string, file: string): string | null {
  const cur = readCurrent(slug)
  if (!cur || cur.status !== 'ready' || !cur.files.includes(file)) return null
  const p = `${appBuildKeyDir(slug, cur.key)}/${file}`
  return existsSync(p) ? p : null
}

export async function loadAppServer(slug: string): Promise<unknown> {
  if (!slugOk(slug)) return null
  const src = appSrc(slug, 'server.ts')
  if (ssrLoad && existsSync(src)) return importFile(src)
  const built = builtFile(slug, 'server.js')
  return built ? importFile(built) : null
}

export async function loadAppMcp(slug: string): Promise<unknown> {
  if (!slugOk(slug)) return null
  const src = appSrc(slug, 'mcp.ts')
  if (ssrLoad && existsSync(src)) return importFile(src)
  const built = builtFile(slug, 'mcp.js')
  return built ? importFile(built) : null
}

export function appHasServer(slug: string): boolean {
  return existsSync(appSrc(slug, 'server.ts')) || builtFile(slug, 'server.js') !== null
}

export function appHasMcp(slug: string): boolean {
  return existsSync(appSrc(slug, 'mcp.ts')) || builtFile(slug, 'mcp.js') !== null
}

/** One module under `apps/<slug>/harnesses/`, not yet imported: each file is
 *  loaded on its own, so a module that throws at import costs that file and
 *  nothing beside it (app-harnesses.ts). */
export interface AppHarnessFile {
  /** The file name under `harnesses/`, e.g. `triage.ts`. */
  file: string
  load: () => Promise<unknown>
}

/** The harness files under `apps/<slug>/harnesses/`, sorted. Dev: each source
 *  file through the Vite SSR graph. Prod: the build's `harnesses.js`, whose
 *  `harnesses` export maps each file name to a lazy import of its own chunk
 *  (build.ts). Empty when the app ships none, or its build is not ready. */
export async function loadAppHarnesses(slug: string): Promise<AppHarnessFile[]> {
  if (!slugOk(slug)) return []
  const dir = appSrc(slug, 'harnesses')
  if (ssrLoad && existsSync(dir)) {
    const load = ssrLoad
    return harnessSourceFiles(dir).map((file) => ({ file, load: () => load(`${dir}/${file}`) }))
  }
  const built = builtFile(slug, 'harnesses.js')
  if (!built) return []
  const mod = (await importFile(built)) as { harnesses?: unknown } | null
  const table = mod?.harnesses
  if (!table || typeof table !== 'object') return []
  return Object.entries(table as Record<string, unknown>)
    .filter((entry): entry is [string, () => Promise<unknown>] => HARNESS_FILE.test(entry[0]) && typeof entry[1] === 'function')
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([file, load]) => ({ file, load }))
}

export function appHasHarnesses(slug: string): boolean {
  return harnessSourceFiles(appSrc(slug, 'harnesses')).length > 0 || builtFile(slug, 'harnesses.js') !== null
}

export async function loadAppClient(slug: string): Promise<unknown> {
  if (!slugOk(slug)) return null
  const src = appSrc(slug, 'app.ts')
  if (ssrLoad && existsSync(src)) return importFile(src)
  return null
}

export function appClientArtifact(slug: string): string | null {
  if (!slugOk(slug)) return null
  return builtFile(slug, 'app.js')
}

export function appHasClient(slug: string): boolean {
  return existsSync(appSrc(slug, 'app.ts')) || appClientArtifact(slug) !== null
}
