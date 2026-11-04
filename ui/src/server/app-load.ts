// Load an app's server/mcp/client module and its harness files. Dev: Vite
// SSR graph (HMR). Prod: the independently-built artifact. Never a host glob
// — the host build does not see apps/.
//
// Every loader reads one TREE: the app's live one by default, or an update's
// staged tree (`apps/.<slug>.next` and its staged build), which the update
// probes before the swap while the live app keeps serving.
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { appDir, HARNESS_FILE, harnessSourceFiles, slugOk } from './app-build/paths'
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

/** One module under `apps/<slug>/harnesses/`, not yet imported: each file is
 *  loaded on its own, so a module that throws at import costs that file and
 *  nothing beside it (app-harnesses.ts). */
export interface AppHarnessFile {
  /** The file name under `harnesses/`, e.g. `triage.ts`. */
  file: string
  load: () => Promise<unknown>
  /** Set when the app build could not compile this file and left it out of
   *  `harnesses.js` (build.ts): the compiler's sentence. `load` rejects with it. */
  compileError?: string
}

/** The harness files under `apps/<slug>/harnesses/`, sorted. Dev: each source
 *  file through the Vite SSR graph. Prod: the build's `harnesses.js`, whose
 *  `harnesses` export maps each file name to a lazy import of its own chunk
 *  and whose `skipped` export names each file the build could not compile,
 *  with the error (build.ts). Empty when the app ships none, or its build is
 *  not ready. */
export async function loadAppHarnesses(slug: string, tree: AppTree = liveTree(slug)): Promise<AppHarnessFile[]> {
  if (!slugOk(slug)) return []
  const dir = join(tree.dir, 'harnesses')
  if (ssrLoad && existsSync(dir)) {
    const load = ssrLoad
    return harnessSourceFiles(dir).map((file) => ({ file, load: () => load(`${dir}/${file}`) }))
  }
  const built = builtFile(tree, 'harnesses.js')
  if (!built) return []
  const mod = (await importFile(built)) as { harnesses?: unknown; skipped?: unknown } | null
  const table = mod?.harnesses
  const skipped = mod?.skipped
  const out: AppHarnessFile[] = []
  if (table && typeof table === 'object') {
    for (const [file, load] of Object.entries(table as Record<string, unknown>)) {
      if (HARNESS_FILE.test(file) && typeof load === 'function') out.push({ file, load: load as () => Promise<unknown> })
    }
  }
  if (skipped && typeof skipped === 'object') {
    for (const [file, error] of Object.entries(skipped as Record<string, unknown>)) {
      if (!HARNESS_FILE.test(file) || typeof error !== 'string' || out.some((f) => f.file === file)) continue
      out.push({ file, compileError: error, load: () => Promise.reject(new Error(error)) })
    }
  }
  return out.sort((a, b) => (a.file < b.file ? -1 : a.file > b.file ? 1 : 0))
}

export function appHasHarnesses(slug: string, tree: AppTree = liveTree(slug)): boolean {
  return harnessSourceFiles(join(tree.dir, 'harnesses')).length > 0 || builtFile(tree, 'harnesses.js') !== null
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
