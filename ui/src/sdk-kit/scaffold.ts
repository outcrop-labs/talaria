// `talaria-app scaffold`: the skeleton `talaria app new` writes, at the root of
// a plugin repo. One source — cli/src/cmd/app/skeleton.ts — for both, so the
// app an agent starts outside the host is the app a person starts inside it.
import { appendFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { basename, dirname, join, resolve } from 'node:path'
import { displayName, harnessStarterFiles, skeletonFiles, standaloneRepoFiles } from '../../../cli/src/cmd/app/skeleton'
import { originRepoName } from './workspace'

export interface ScaffoldOptions {
  name?: string
  icon?: string
  harness?: boolean
}

export interface ScaffoldResult {
  dir: string
  written: string[]
  /** Files the repo already had and kept (a README, a .gitignore that got
   *  the kit's lines appended, a seed talaria.json the skeleton filled in). */
  kept: string[]
}

/** The display name a checkout suggests: its repository's name (the origin
 *  remote's, else the directory's) without the `talaria-app-` prefix install
 *  strips, title-cased. A workbench job clones into a directory named for the
 *  job, so the directory alone would name the app after the job. */
export function nameForDir(dir: string): string {
  const repo = (originRepoName(dir) ?? basename(resolve(dir))).replace(/\.git$/, '').toLowerCase()
  return displayName(repo.replace(/^talaria-app-/, '')) || 'My App'
}

/** A talaria.json that is the only part of the skeleton in the directory: a
 *  SEED. A new repo's first file is one, written so that `prepare_env` —
 *  which keys on talaria.json and nothing else — installs the kit that
 *  scaffolds the rest. Its object, or why it cannot be one. */
function readSeed(path: string): Record<string, unknown> | string {
  let value: unknown
  try {
    value = JSON.parse(readFileSync(path, 'utf8'))
  } catch (e) {
    return `${path} is not JSON (${e instanceof Error ? e.message : String(e)}) — a seed talaria.json is an object such as {"name": "Notes"}`
  }
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    return `${path} is not a JSON object — a seed talaria.json is an object such as {"name": "Notes"}`
  }
  return value as Record<string, unknown>
}

const text = (v: unknown): string | undefined => (typeof v === 'string' && v.trim() ? v : undefined)

/** Write the app into `dir`. Refuses a directory that already holds an app
 *  (a talaria.json beside any other file of the skeleton) or any file the
 *  skeleton would overwrite; a repo's own README is kept and its .gitignore
 *  extended, never replaced. A talaria.json alone is a seed: every key it
 *  declares is kept over the skeleton's (its name and icon too, unless
 *  --name or --icon says otherwise), and the rest of the app is written
 *  around it. */
export function scaffoldApp(dir: string, opts: ScaffoldOptions = {}): ScaffoldResult | string {
  const root = resolve(dir)
  const manifestPath = join(root, 'talaria.json')
  const draft = { ...skeletonFiles({ name: 'x', icon: 'x' }), ...(opts.harness ? harnessStarterFiles({ name: 'x' }) : {}) }
  let seed: Record<string, unknown> | null = null
  if (existsSync(manifestPath)) {
    if (Object.keys(draft).some((f) => f !== 'talaria.json' && existsSync(join(root, f)))) return `${root} already holds an app (talaria.json)`
    const read = readSeed(manifestPath)
    if (typeof read === 'string') return read
    seed = read
  }
  const name = (opts.name ?? text(seed?.name) ?? nameForDir(root)).trim()
  if (!name) return '--name cannot be empty'
  const icon = (opts.icon ?? text(seed?.icon) ?? '⬡').trim() || '⬡'
  const app = { ...skeletonFiles({ name, icon }), ...(opts.harness ? harnessStarterFiles({ name }) : {}) }
  const taken = Object.keys(app).filter((f) => existsSync(join(root, f)) && !(seed && f === 'talaria.json'))
  if (taken.length) return `${root} already has ${taken.join(', ')} — scaffold into an empty directory or a fresh repository`
  if (seed) {
    const manifest = { ...(JSON.parse(app['talaria.json']!) as Record<string, unknown>), ...seed, name, icon }
    app['talaria.json'] = `${JSON.stringify(manifest, null, 2)}\n`
  }
  const written: string[] = []
  const kept: string[] = []
  mkdirSync(root, { recursive: true })
  for (const [rel, body] of Object.entries(app)) {
    mkdirSync(dirname(join(root, rel)), { recursive: true })
    writeFileSync(join(root, rel), body)
    ;(seed && rel === 'talaria.json' ? kept : written).push(rel)
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
