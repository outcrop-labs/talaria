// Shared specifiers between the host build and a separately-compiled app.
// One list drives both the host's `runtime/rt-*.js` entries and the app
// builder's `external` map, so a Svelte bump cannot silently leave a
// specifier unmapped (that failure mode is `effect_orphan` at runtime).
import { readFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { join } from 'node:path'

const SVELTE_EXCLUDE = new Set([
  '.',
  './package.json',
  './action',
  './compiler',
  './elements',
  './internal/server',
  './server',
])

function sveltePackage(): { version: string; exports: Record<string, unknown> } {
  const raw = readFileSync(join(process.cwd(), 'node_modules/svelte/package.json'), 'utf8')
  return JSON.parse(raw) as { version: string; exports: Record<string, unknown> }
}

/** Runtime svelte subpaths derived from the installed package's exports map. */
export function svelteClientSpecifiers(): string[] {
  const out = ['svelte']
  for (const key of Object.keys(sveltePackage().exports)) {
    if (SVELTE_EXCLUDE.has(key) || key.includes('compiler')) continue
    out.push(`svelte${key.slice(1)}`)
  }
  return out.sort()
}

/** Specifiers an app client chunk must import from the host, never bundle. */
export function clientSpecifiers(): string[] {
  return [...svelteClientSpecifiers(), '@talaria/sdk', '@lucide/svelte', '@tanstack/svelte-query']
}

/** The packages an app's server-side code imports by name besides the SDK,
 *  each by its bare entry only (no subpath). zod is the copy
 *  `@talaria/sdk/server` re-exports as `z`, so a schema an app builds with
 *  either is one zod's. main's SDK shared it, and apps written then import it
 *  directly (`import { z } from 'zod'` in a lib/schema.ts that the app's
 *  plain-`bun test` half also imports). */
export const SHARED_SERVER_PACKAGES: readonly string[] = ['zod']

/** Specifiers an app server/mcp chunk must import from the host, never bundle. */
export function serverSpecifiers(): string[] {
  return ['@talaria/sdk/server', ...SHARED_SERVER_PACKAGES]
}

/** Stable filename for a specifier: `@talaria/sdk` → `rt-talaria-sdk`. */
export function shimFileName(specifier: string): string {
  return `rt-${specifier.replace(/^@/, '').replace(/[/.]/g, '-')}`
}

/** Shared modules whose default export an app may import too: `export *`
 *  carries every named export but never `default`, and zod's default is the
 *  `z` namespace (`import z from 'zod'`). */
const DEFAULT_EXPORTS: ReadonlySet<string> = new Set(['zod'])

/** The source of the host's runtime shim for `spec`: everything the module
 *  exports (its default included, when it has one) and the module itself
 *  evaluated. `from` is what the shim imports — the specifier, unless a build
 *  names the file. */
export function shimSource(spec: string, from: string = spec): string {
  const id = JSON.stringify(from)
  return `export * from ${id};\n${DEFAULT_EXPORTS.has(spec) ? `export { default } from ${id};\n` : ''}import ${id};\n`
}

/** Why `id`, a subpath of a shared package or no shared package at all, has
 *  no runtime file. */
function unsharedSubpath(id: string, shared: readonly string[]): Error {
  const pkg = shared.filter((s) => id.startsWith(`${s}/`)).sort((a, b) => b.length - a.length)[0]
  return new Error(
    pkg ? `"${id}" is not shared with apps; the host shares "${pkg}" and its listed subpaths only` : `"${id}" is not shared with apps`,
  )
}

/** The host runtime file an app client chunk imports for `id`. Every shared
 *  specifier has its own `runtime/rt-*.js` entry (vite-plugin.ts), so the
 *  match is exact: `svelte/internal/client`, which every compiled component
 *  imports, is its own shim. A first-prefix match sent it to `svelte`'s
 *  rt-svelte.js, which has no `from_html`, and the app failed in the browser.
 *  A subpath of a shared package that is not itself shared has no shim, so
 *  this throws and the build fails naming the import. */
export function clientRuntimePath(id: string): string {
  const shared = clientSpecifiers()
  if (shared.includes(id)) return `/runtime/${shimFileName(id)}.js`
  throw unsharedSubpath(id, shared)
}

/** The host runtime file, under the server build's `runtime/`, an app's
 *  server, mcp or harness chunk imports for `id`: one per server specifier,
 *  matched exactly, as on the client. A subpath of a shared package that is
 *  not itself shared (`zod/mini`) has no file, so this throws and the build
 *  fails naming the import. */
export function serverRuntimeFile(id: string): string {
  const shared = serverSpecifiers()
  if (shared.includes(id)) return `${shimFileName(id)}.js`
  throw unsharedSubpath(id, shared)
}

export function isClientSpecifier(id: string): boolean {
  return clientSpecifiers().some((s) => id === s || id.startsWith(`${s}/`))
}

export function isServerSpecifier(id: string): boolean {
  return serverSpecifiers().some((s) => id === s || id.startsWith(`${s}/`))
}

/** Host half of the app-build cache key. A host upgrade invalidates every app. */
export function hostBuildStamp(): string {
  const ver = process.env.TALARIA_VERSION
  if (ver && ver !== 'unknown') return ver
  const h = createHash('sha256')
  h.update(clientSpecifiers().join('\n'))
  h.update('\n')
  h.update(sveltePackage().version)
  return `dev-${h.digest('hex').slice(0, 12)}`
}

/** Module ids that mean a shared specifier leaked into an app chunk. */
export function bundledShared(moduleIds: string[]): string[] {
  const marks = [
    '/node_modules/svelte/',
    '/src/sdk/',
    '/node_modules/@lucide/svelte/',
    '/node_modules/@tanstack/svelte-query/',
    ...SHARED_SERVER_PACKAGES.map((p) => `/node_modules/${p}/`),
  ]
  return moduleIds.filter((id) => {
    const n = id.replace(/\\/g, '/')
    return marks.some((m) => n.includes(m))
  })
}
