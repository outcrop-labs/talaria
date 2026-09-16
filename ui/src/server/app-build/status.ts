import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { appBuildsDir } from './paths'

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
