// The app host's half of an app update. The api (talaria-apps `update_app`)
// fetches the new commit into apps/.<slug>.next and drives these steps over
// /api/app-host/<slug>/lifecycle:
//
//   stage     build the staged tree beside the live build (current.json
//             untouched) and probe it from there — the live app keeps
//             serving, and a version that does not build or load stops here
//   activate  after the api swapped the trees (renameat2 RENAME_EXCHANGE),
//             point the app at the staged build and probe it live; on
//             failure the previous pointer is restored before answering, so
//             the api's swap back finds its build where it left it
//   release   let go of the app without switching (the api gave up), and
//             discard the build stage made: it will never be live
//
// Between stage and activate the app is HELD: the boot sweep, the directory
// watcher and Rebuild leave it alone, so nothing builds or fails the tree
// that is about to be swapped, and nothing turns the app off over a version
// that is not live yet. A hold lapses on its own after HOLD_MS, should the
// api never come back.
import { existsSync, rmSync } from 'node:fs'
import { buildAppAt } from './build'
import { appBuildKeyDir, appDir, appNextDir, slugOk } from './paths'
import { readCurrent, writeCurrent, pruneSupersededBuilds, type AppBuildCurrent } from './status'
import { sourceKey } from './key'
import { invalidateLoaded, type AppTree } from '../app-load'
import { probeLoad } from '../app-health'
import { isolateApp } from '../app-isolate'

/** How long a stage may wait for its activate. A build takes seconds to
 *  minutes; the api's own deadline for one step is ten. */
export const HOLD_MS = 15 * 60_000

interface Hold {
  until: number
  staged: AppBuildCurrent | null
}

// On globalThis: dev serves routes from Vite's SSR graph, which can hand two
// requests two instances of this module after an edit.
const g = globalThis as { __talariaAppHolds?: Map<string, Hold> }
const holds = (g.__talariaAppHolds ??= new Map<string, Hold>())

/** Whether an update holds `slug` right now. */
export function isHeld(slug: string): boolean {
  const h = holds.get(slug)
  if (!h) return false
  if (h.until > Date.now()) return true
  holds.delete(slug)
  return false
}

/** Let go of the app without switching: the api gave up after stage (the
 *  swap failed, or its call never got an answer). The hold ends, and the
 *  build stage made is removed — it will never be live, and nothing else
 *  would ever remove it. */
export function release(slug: string): void {
  const hold = holds.get(slug)
  holds.delete(slug)
  if (hold?.staged) discardStaged(slug, hold.staged)
}

type Step = { ok: true; build: AppBuildCurrent } | { ok: false; error: string; build?: AppBuildCurrent }

/** A staged build that will never be live: removed, unless it is the live
 *  build itself (the new version had the same files). */
function discardStaged(slug: string, build: AppBuildCurrent | undefined): void {
  if (!build?.key || build.key === readCurrent(slug)?.key) return
  try {
    rmSync(appBuildKeyDir(slug, build.key), { recursive: true, force: true })
  } catch (e) {
    console.error(`[app-update] ${slug}: could not remove the staged build ${build.key}`, e)
  }
}

/** Build and probe `apps/.<slug>.next` without touching the live app. */
export async function stageUpdate(slug: string): Promise<Step> {
  if (!slugOk(slug)) return { ok: false, error: 'bad slug' }
  const dir = appNextDir(slug)
  if (!existsSync(dir)) return { ok: false, error: `no staged version at ${dir}` }
  holds.set(slug, { until: Date.now() + HOLD_MS, staged: null })
  const built = await isolateApp(slug, 'build staged update', () => buildAppAt(slug, dir, { pointer: false }))
  if (!built.ok || built.value.status !== 'ready') {
    release(slug)
    if (built.ok) discardStaged(slug, built.value)
    const error = built.ok ? (built.value.error ?? 'the new version failed to compile') : built.error
    return { ok: false, error, ...(built.ok ? { build: built.value } : {}) }
  }
  const tree: AppTree = { slug, dir, build: built.value }
  const probed = await probeLoad(slug, { tree, catalog: false })
  invalidateLoaded(dir)
  if (!probed.ok) {
    release(slug)
    discardStaged(slug, built.value)
    return { ok: false, error: probed.error, build: built.value }
  }
  holds.set(slug, { until: Date.now() + HOLD_MS, staged: built.value })
  return { ok: true, build: built.value }
}

/** After the swap: point the app at the staged build and probe it live. The
 *  hold is released either way. */
export async function activateUpdate(slug: string): Promise<Step> {
  if (!slugOk(slug)) return { ok: false, error: 'bad slug' }
  const hold = holds.get(slug)
  // The hold ends here, but the staged build is the one being switched to:
  // not `release`, which would discard it.
  holds.delete(slug)
  const staged = hold?.staged
  if (!staged) return { ok: false, error: 'no staged build to activate (it was released or expired)' }
  const live = appDir(slug)
  // The swapped-in tree must be the one that was built: same files, same key.
  if (sourceKey(live, staged.hostBuild) !== staged.key) {
    return { ok: false, error: 'the live tree is not the staged version' }
  }
  const previous = readCurrent(slug)
  writeCurrent(slug, staged)
  // Dev: the SSR graph holds the old modules under these paths (and the
  // staged ones under .next, which now holds the old tree).
  invalidateLoaded(live)
  invalidateLoaded(appNextDir(slug))
  const probed = await probeLoad(slug)
  if (!probed.ok) {
    if (previous) writeCurrent(slug, previous)
    invalidateLoaded(live)
    discardStaged(slug, staged)
    return { ok: false, error: probed.error, build: staged }
  }
  pruneSupersededBuilds(slug)
  return { ok: true, build: staged }
}
