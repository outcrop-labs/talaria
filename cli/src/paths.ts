// Path and port plumbing. The recurring, load-bearing theme is CANONICAL
// PATHS: this host keeps /home/jon/Development as a symlink to
// /mnt/data/Development, and the logical spelling must never leak into
// compose.env, a box's ui/.env, or the same-path state bind — inside a box
// /home/jon does not exist, so a symlinked host path breaks the mount. Node's
// recursive mkdir has the same allergy: it walks RAW path segments, so a `..`
// that resolves fine on the host tries to create a segment before it that
// does not exist in the box. Hence: realpath everything that crosses a
// container boundary, and canonicalize (create + realpath) directories that
// node will mkdir.

import { existsSync, mkdirSync, readdirSync, realpathSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import net from 'node:net'

/** Find the repo root by walking up from a starting directory to the first
 *  .git. The start should itself already be realpath'd. */
export function repoRoot(start: string): string {
  let dir = start
  for (let i = 0; i < 10; i++) {
    if (existsSync(join(dir, '.git'))) return dir
    const parent = dirname(dir)
    if (parent === dir) break
    dir = parent
  }
  throw new Error(`no repo root above ${start}`)
}

/** Create-if-missing and resolve to the physical path — the
 *  `mkdir -p … && cd … && pwd -P` idiom. */
export function canonicalDir(p: string): string {
  mkdirSync(p, { recursive: true })
  return realpathSync(p)
}

/** The devbox registry root, canonicalized (see the module comment for why
 *  a `..`-bearing or symlinked spelling is not allowed to survive). */
export function devboxHome(root: string, env: Record<string, string | undefined>): string {
  return canonicalDir(env.TALARIA_DEVBOX_HOME ?? join(root, '..', 'devboxes'))
}

export const boxDir = (devboxes: string, name: string): string => join(devboxes, name)

/** Lowercase-kebab box/worktree names — they land in container names, docker
 *  networks and directory names; anything else is a typo waiting to hurt. */
export const NAME_RE = /^[a-z0-9][a-z0-9-]*$/

/** A port is free when a connect attempt is REFUSED. (Success means
 *  something is listening — the bash /dev/tcp probe, as a promise.) */
export function portTaken(port: number, host = '127.0.0.1'): Promise<boolean> {
  return new Promise((resolve) => {
    const s = net.connect({ port, host })
    s.once('connect', () => {
      s.destroy()
      resolve(true)
    })
    s.once('error', () => resolve(false))
  })
}

/** First free port in [lo, hi], or null — the worktree.sh slot scan. The
 *  probe is injectable so tests can stage occupied ranges. */
export async function portSlot(
  lo: number,
  hi: number,
  taken: (port: number) => Promise<boolean> = (p) => portTaken(p),
): Promise<number | null> {
  for (let p = lo; p <= hi; p++) {
    if (!(await taken(p))) return p
  }
  return null
}

/** mtime comparison: does `candidate` exist and postdate `ref`? The
 *  `find -newer` equivalent — a checkout of an OLDER file still trips it,
 *  because mtime is the checkout time. */
export function isNewer(candidate: string, ref: string): boolean {
  if (!existsSync(candidate) || !existsSync(ref)) return false
  return statSync(candidate).mtimeMs > statSync(ref).mtimeMs
}

/** Does ANY file under `dir` (recursively) postdate `ref`? The
 *  `find dir -newer ref -print -quit` staleness probe, first match wins.
 *  `dir` missing simply means nothing is newer. */
export function anyNewer(dir: string, ref: string): boolean {
  if (!existsSync(dir) || !existsSync(ref)) return false
  const refMs = statSync(ref).mtimeMs
  const walk = (d: string): boolean => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name)
      if (e.isDirectory()) {
        if (walk(p)) return true
      } else if (statSync(p).mtimeMs > refMs) {
        return true
      }
    }
    return false
  }
  return walk(dir)
}
