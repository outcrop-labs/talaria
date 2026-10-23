// Cache key for one app build: hash of its sources, the host build id, the
// shared-specifier list, and the build recipe. A host upgrade, an SDK change,
// a change to how build.ts compiles, and a git pull in the app each
// invalidate exactly what they must.
import { createHash } from 'node:crypto'
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { clientSpecifiers, serverSpecifiers } from '@/app-runtime/specifiers'

/** Version of build.ts's output contract. Bump it in the same change
 *  whenever build.ts changes what a build emits or how the host reads it:
 *  Vite/Rollup options, the externals and their runtime paths, the artifact
 *  names. Without TALARIA_VERSION (dev and checkout installs) the host build
 *  id does not move when build.ts does, so a build the old recipe left
 *  'ready' would keep its key and be reused as is. History:
 *  1. the first recipe (no recipe in the key);
 *  2. app.js keeps its default export (preserveEntrySignatures 'strict') and
 *     each shared import maps to its own runtime file (clientRuntimePath);
 *  3. harnesses/*.ts compile into harnesses.js, one lazy chunk per file
 *     under _harnesses/, which the host registers for fitness. */
export const BUILD_RECIPE = 3

const SKIP: Record<string, true> = { node_modules: true, '.git': true, dist: true }

function walk(dir: string, files: string[]): void {
  if (!existsSync(dir)) return
  for (const name of readdirSync(dir).sort()) {
    if (SKIP[name] || name.startsWith('.')) continue
    const p = join(dir, name)
    const st = statSync(p)
    if (st.isDirectory()) walk(p, files)
    else files.push(p)
  }
}

export function sourceKey(appDir: string, hostBuild: string, recipe: number = BUILD_RECIPE): string {
  const h = createHash('sha256')
  h.update(`recipe:${recipe}`)
  h.update('\0')
  h.update(hostBuild)
  h.update('\0')
  h.update(clientSpecifiers().join('\n'))
  h.update('\0')
  h.update(serverSpecifiers().join('\n'))
  const files: string[] = []
  walk(appDir, files)
  for (const file of files) {
    h.update(relative(appDir, file))
    h.update('\0')
    h.update(readFileSync(file))
    h.update('\0')
  }
  return h.digest('hex').slice(0, 16)
}
