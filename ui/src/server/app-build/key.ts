// Cache key for one app build: hash of its sources, the host build id, and
// the shared-specifier list. A host upgrade, an SDK change, and a git pull
// in the app each invalidate exactly what they must.
import { createHash } from 'node:crypto'
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { clientSpecifiers, serverSpecifiers } from '@/app-runtime/specifiers'

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

export function sourceKey(appDir: string, hostBuild: string): string {
  const h = createHash('sha256')
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
