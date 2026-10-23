// Load an app's server/mcp/client module. Dev: Vite SSR graph (HMR). Prod:
// the independently-built artifact. Never a host glob — the host build does
// not see apps/.
//
// Every loader reads one TREE: the app's live one by default, or an update's
// staged tree (`apps/.<slug>.next` and its staged build), which the update
// probes before the swap while the live app keeps serving.
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { appDir, slugOk } from './app-build/paths'
import { readCurrent, type AppBuildCurrent } from './app-build/status'
import { appBuildKeyDir } from './app-build/paths'

/** One app's code: its source directory and the build that stands for it. */
export interface AppTree {
  slug: string
  dir: string
  build: AppBuildCurrent | null
}

/** The app as it is installed: `apps/<slug>` and its current build. */
export function liveTree(slug: string): AppTree {
  return { slug, dir: appDir(slug), build: readCurrent(slug) }
}

let ssrLoad: ((abs: string) => Promise<unknown>) | null = null
let ssrInvalidate: ((dir: string) => void) | null = null

export function setSsrLoader(fn: ((abs: string) => Promise<unknown>) | null): void {
  ssrLoad = fn
}

/** Dev only: how to drop every module Vite holds from under one directory.
 *  An update swaps the whole apps/<slug> tree in one rename, and the SSR
 *  graph keyed by those paths would otherwise keep serving the old code. */
export function setSsrInvalidator(fn: ((dir: string) => void) | null): void {
  ssrInvalidate = fn
}

/** Forget what the dev server loaded from `dir` (a no-op in production,
 *  where a new build is a new path). */
export function invalidateLoaded(dir: string): void {
  ssrInvalidate?.(dir)
}

async function importFile(abs: string): Promise<unknown> {
  if (!existsSync(abs)) return null
  if (ssrLoad) return ssrLoad(abs)
  return import(pathToFileURL(abs).href)
}

function builtFile(tree: AppTree, file: string): string | null {
  const cur = tree.build
  if (!cur || cur.status !== 'ready' || !cur.files.includes(file)) return null
  const p = `${appBuildKeyDir(tree.slug, cur.key)}/${file}`
  return existsSync(p) ? p : null
}

export async function loadAppServer(slug: string, tree: AppTree = liveTree(slug)): Promise<unknown> {
  if (!slugOk(slug)) return null
  const src = join(tree.dir, 'server.ts')
  if (ssrLoad && existsSync(src)) return importFile(src)
  const built = builtFile(tree, 'server.js')
  return built ? importFile(built) : null
}

export async function loadAppMcp(slug: string, tree: AppTree = liveTree(slug)): Promise<unknown> {
  if (!slugOk(slug)) return null
  const src = join(tree.dir, 'mcp.ts')
  if (ssrLoad && existsSync(src)) return importFile(src)
  const built = builtFile(tree, 'mcp.js')
  return built ? importFile(built) : null
}

export function appHasServer(slug: string, tree: AppTree = liveTree(slug)): boolean {
  return existsSync(join(tree.dir, 'server.ts')) || builtFile(tree, 'server.js') !== null
}

export function appHasMcp(slug: string, tree: AppTree = liveTree(slug)): boolean {
  return existsSync(join(tree.dir, 'mcp.ts')) || builtFile(tree, 'mcp.js') !== null
}

export async function loadAppClient(slug: string, tree: AppTree = liveTree(slug)): Promise<unknown> {
  if (!slugOk(slug)) return null
  const src = join(tree.dir, 'app.ts')
  if (ssrLoad && existsSync(src)) return importFile(src)
  return null
}

export function appClientArtifact(slug: string, tree: AppTree = liveTree(slug)): string | null {
  if (!slugOk(slug)) return null
  return builtFile(tree, 'app.js')
}

export function appHasClient(slug: string, tree: AppTree = liveTree(slug)): boolean {
  return existsSync(join(tree.dir, 'app.ts')) || appClientArtifact(slug, tree) !== null
}
