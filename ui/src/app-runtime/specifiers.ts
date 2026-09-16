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

/** Specifiers an app server/mcp chunk must import from the host, never bundle. */
export function serverSpecifiers(): string[] {
  return ['@talaria/sdk/server']
}

/** Stable filename for a specifier: `@talaria/sdk` → `rt-talaria-sdk`. */
export function shimFileName(specifier: string): string {
  return `rt-${specifier.replace(/^@/, '').replace(/[/.]/g, '-')}`
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
  const marks = ['/node_modules/svelte/', '/src/sdk/', '/node_modules/@lucide/svelte/', '/node_modules/@tanstack/svelte-query/']
  return moduleIds.filter((id) => {
    const n = id.replace(/\\/g, '/')
    return marks.some((m) => n.includes(m))
  })
}
