// `talaria-app scaffold`: the skeleton `talaria app new` writes, at the root of
// a plugin repo. One source — cli/src/cmd/app/skeleton.ts — for both, so the
// app an agent starts outside the host is the app a person starts inside it.
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, dirname, join, resolve } from 'node:path'
import { displayName, harnessStarterFiles, skeletonFiles, standaloneRepoFiles } from '../../../cli/src/cmd/app/skeleton'

export interface ScaffoldOptions {
  name?: string
  icon?: string
  harness?: boolean
}

export interface ScaffoldResult {
  dir: string
  written: string[]
  /** Files the repo already had and kept (a README, a .gitignore that got
   *  the kit's lines appended). */
  kept: string[]
}

/** The display name a directory suggests: the repo's name without the
 *  `talaria-app-` prefix install strips, title-cased. */
export function nameForDir(dir: string): string {
  return displayName(basename(resolve(dir)).replace(/^talaria-app-/, '')) || 'My App'
}

/** Write the app into `dir`. Refuses a directory that already holds an app
 *  (talaria.json) or any file the skeleton would overwrite; a repo's own
 *  README is kept and its .gitignore extended, never replaced. */
export function scaffoldApp(dir: string, opts: ScaffoldOptions = {}): ScaffoldResult | string {
  const root = resolve(dir)
  if (existsSync(join(root, 'talaria.json'))) return `${root} already holds an app (talaria.json)`
  const name = (opts.name ?? nameForDir(root)).trim()
  if (!name) return '--name cannot be empty'
  const icon = (opts.icon ?? '⬡').trim() || '⬡'
  const app = { ...skeletonFiles({ name, icon }), ...(opts.harness ? harnessStarterFiles({ name }) : {}) }
  const taken = Object.keys(app).filter((f) => existsSync(join(root, f)))
  if (taken.length) return `${root} already has ${taken.join(', ')} — scaffold into an empty directory or a fresh repository`
  const written: string[] = []
  const kept: string[] = []
  mkdirSync(root, { recursive: true })
  for (const [rel, body] of Object.entries(app)) {
    mkdirSync(dirname(join(root, rel)), { recursive: true })
    writeFileSync(join(root, rel), body)
    written.push(rel)
  }
  for (const [rel, body] of Object.entries(standaloneRepoFiles({ name }))) {
    const p = join(root, rel)
    if (!existsSync(p)) {
      writeFileSync(p, body)
      written.push(rel)
      continue
    }
    if (rel === '.gitignore') {
      const have = new Set(readFileSync(p, 'utf8').split('\n').map((l) => l.trim()))
      const missing = body.split('\n').filter((l) => l.trim() && !l.startsWith('#') && !have.has(l.trim()))
      if (missing.length) appendFileSync(p, `${readFileSync(p, 'utf8').endsWith('\n') ? '' : '\n'}${missing.join('\n')}\n`)
    }
    kept.push(rel)
  }
  return { dir: root, written, kept }
}
