// Where the kit runs the host's code, and how it points that code at an app
// that is not installed anywhere.
//
// The host's build recipe, load probe and harness listing read their places
// from the process: the apps, builds and data directories from env
// (app-build/paths.ts), and their own root — the stylesheet an app's CSS
// compiles against, the svelte config, the installed svelte, the SDK shim a
// server chunk links to — from the working directory. So the kit sets the
// three directories to a work area of its own, outside the app's tree, and
// runs from its own root. The app's source is read where it is, never copied.
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, realpathSync, statSync } from 'node:fs'
import { homedir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { RESERVED_APP_SLUGS, SLUG_RE, validateManifest, type AppManifest } from '@/server/app-build/manifest'
import { KIT_FORMAT, KIT_PATHS, type KitManifest } from './kit'

/** The directory at or above `start` holding a talaria.json: the app. */
export function findAppDir(start: string): string | null {
  let dir = resolve(start)
  for (;;) {
    if (existsSync(join(dir, 'talaria.json'))) return dir
    const up = dirname(dir)
    if (up === dir) return null
    dir = up
  }
}

/** The repository name the checkout at `dir` came from: the last segment of
 *  its `origin` remote, read out of .git/config. A workbench job clones into
 *  a directory named for the job, so the directory's own name says nothing
 *  about the repo an install will read. Null without a checkout or a remote. */
export function originRepoName(dir: string): string | null {
  let config: string
  try {
    config = readFileSync(join(dir, '.git', 'config'), 'utf8')
  } catch {
    return null
  }
  const section = /\[remote "origin"\]([^[]*)/.exec(config)?.[1]
  const url = section ? /^\s*url\s*=\s*(\S+)\s*$/m.exec(section)?.[1] : undefined
  const name = url?.replace(/\/+$/, '').split(/[/:]/).pop()
  return name ? name : null
}

/** The slug the kit builds the app as. The one talaria.json declares, else
 *  the one an install of its repository would take (talaria-apps
 *  `install_slug`: the repo's name, lowercased, without `.git` or a
 *  `talaria-app-` prefix) — the origin remote's name when the checkout has
 *  one, else the directory's. Null when that is not a slug an install would
 *  accept. */
export function slugFor(dir: string, manifest: Pick<AppManifest, 'slug'> | null, override?: string): string | null {
  const repo = originRepoName(dir) ?? basename(dir)
  const candidate = (override ?? manifest?.slug ?? repo.replace(/\.git$/, '').toLowerCase().replace(/^talaria-app-/, '')).trim()
  return SLUG_RE.test(candidate) && !RESERVED_APP_SLUGS.includes(candidate) ? candidate : null
}

/** The manifest at `dir`, held to the host's rules, read as bytes the way the
 *  host reads it. */
export function readAppManifest(dir: string, hostSdk: string): { ok: true; manifest: AppManifest } | { ok: false; error: string } {
  let raw: string
  try {
    raw = readFileSync(join(dir, 'talaria.json'), 'utf8')
  } catch (e) {
    return { ok: false, error: `talaria.json could not be read: ${e instanceof Error ? e.message : String(e)}` }
  }
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch (e) {
    return { ok: false, error: `talaria.json is not JSON: ${e instanceof Error ? e.message : String(e)}` }
  }
  return validateManifest(value, hostSdk)
}

/** A work area per app, keyed by the app's real path, under the user's cache
 *  — never inside the app's tree, where the stylesheet's `@source` scan and
 *  the build key's file walk would both read it. */
export function workDirFor(appDir: string, env: NodeJS.ProcessEnv = process.env): string {
  const real = realpathSync(appDir)
  const digest = createHash('sha256').update(real).digest('hex').slice(0, 12)
  const cache = env.XDG_CACHE_HOME?.trim() || join(env.HOME?.trim() || homedir(), '.cache')
  return join(cache, 'talaria-app', `${basename(real)}-${digest}`)
}

export interface KitWorkspace {
  /** The kit's root: node_modules, the shim, the stylesheet. */
  kit: string
  /** The app's real directory. */
  app: string
  slug: string
  /** The work area: `apps/` (empty), `app-builds/`, `app-data/`. */
  work: string
  builds: string
}

/** Point the host's code at `app`, built as `slug`, from the kit at `kit`.
 *  Process-wide: it sets env and the working directory, which is what the
 *  host's modules read. */
export function enterWorkspace(kit: string, app: string, slug: string, env: NodeJS.ProcessEnv = process.env): KitWorkspace {
  const real = realpathSync(app)
  const work = workDirFor(real, env)
  const apps = join(work, 'apps')
  const builds = join(work, 'app-builds')
  const data = join(work, 'app-data')
  for (const d of [apps, builds, data]) mkdirSync(d, { recursive: true })
  env.TALARIA_APPS_DIR = apps
  env.TALARIA_APP_BUILDS_DIR = builds
  env.TALARIA_APP_DATA_DIR = data
  process.chdir(kit)
  return { kit, app: real, slug, work, builds }
}

/** kit.json at `kit`, or why it is not a kit this CLI can use. */
export function readKitManifest(kit: string): KitManifest | string {
  const p = join(kit, KIT_PATHS.manifest)
  if (!existsSync(p) || !statSync(p).isFile()) return `${kit} is not an SDK kit (no ${KIT_PATHS.manifest})`
  try {
    const m = JSON.parse(readFileSync(p, 'utf8')) as Partial<KitManifest>
    if (m.format !== KIT_FORMAT) return `${p} is kit format ${String(m.format)}; this CLI reads format ${KIT_FORMAT}`
    if (typeof m.version !== 'string' || typeof m.sdk !== 'string') return `${p} names no version`
    return m as KitManifest
  } catch (e) {
    return `${p} could not be read: ${e instanceof Error ? e.message : String(e)}`
  }
}
