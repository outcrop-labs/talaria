// Load an app's server/mcp/client module. Dev: Vite SSR graph (HMR). Prod:
// the independently-built artifact. Never a host glob — the host build does
// not see apps/.
import { existsSync } from 'node:fs'
import { pathToFileURL } from 'node:url'
import { appSrc, slugOk } from './app-build/paths'
import { readCurrent } from './app-build/status'
import { appBuildKeyDir } from './app-build/paths'

let ssrLoad: ((abs: string) => Promise<unknown>) | null = null

export function setSsrLoader(fn: (abs: string) => Promise<unknown>): void {
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
