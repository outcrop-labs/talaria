import { existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { appBuildsDir, buildKeyOk } from './paths'

export type AppBuildStatus = 'ready' | 'building' | 'failed'

export interface AppBuildCurrent {
  key: string
  status: AppBuildStatus
  hostBuild: string
  builtAt: number
  bytes: number
  files: string[]
  error?: string
}

export function currentPath(slug: string): string {
  return join(appBuildsDir(), slug, 'current.json')
}

export function readCurrent(slug: string): AppBuildCurrent | null {
  const p = currentPath(slug)
  if (!existsSync(p)) return null
  try {
    const v = JSON.parse(readFileSync(p, 'utf8')) as Partial<AppBuildCurrent>
    if (v.status !== 'ready' && v.status !== 'building' && v.status !== 'failed') return null
    return {
      key: typeof v.key === 'string' ? v.key : '',
      status: v.status,
      hostBuild: typeof v.hostBuild === 'string' ? v.hostBuild : '',
      builtAt: typeof v.builtAt === 'number' ? v.builtAt : 0,
      bytes: typeof v.bytes === 'number' ? v.bytes : 0,
      files: Array.isArray(v.files) ? v.files.filter((f): f is string => typeof f === 'string') : [],
      ...(typeof v.error === 'string' ? { error: v.error } : {}),
    }
  } catch {
    return null
  }
}

export function writeCurrent(slug: string, cur: AppBuildCurrent): void {
  const dir = join(appBuildsDir(), slug)
  mkdirSync(dir, { recursive: true })
  const tmp = join(dir, 'current.json.tmp')
  writeFileSync(tmp, `${JSON.stringify(cur, null, 2)}\n`)
  renameSync(tmp, currentPath(slug))
}

/** The build the /app-builds/ door may serve for `slug`: the key the current
 *  pointer names, and only once that build is ready. Null while it builds,
 *  after it failed, or when there is no pointer. */
export function servableBuildKey(slug: string): string | null {
  const cur = readCurrent(slug)
  return cur?.status === 'ready' && buildKeyOk(cur.key) ? cur.key : null
}

/** Remove every build-key directory of `slug` that the current pointer does
 *  not name, once the pointer names a READY build. A superseded build is
 *  never served (the door serves only the current key), and builds made
 *  before the server and MCP chunks moved to `_server/` and `_mcp/` keep
 *  those chunks under `assets/` — so they go, rather than sit on disk beside
 *  the build that replaced them. Only directories named like a key are
 *  touched (never current.json or its .tmp), and a directory that cannot be
 *  removed is logged and left: pruning never fails a build. Returns the keys
 *  removed. */
export function pruneSupersededBuilds(slug: string): string[] {
  const keep = servableBuildKey(slug)
  if (keep === null) return []
  const dir = join(appBuildsDir(), slug)
  let names: string[]
  try {
    names = readdirSync(dir)
  } catch {
    return []
  }
  const removed: string[] = []
  for (const name of names) {
    if (name === keep || !buildKeyOk(name)) continue
    const p = join(dir, name)
    try {
      if (!lstatSync(p).isDirectory()) continue
      rmSync(p, { recursive: true, force: true })
      removed.push(name)
    } catch (e) {
      console.error(`[app-build] ${slug}: could not remove superseded build ${name}`, e)
    }
  }
  return removed
}
