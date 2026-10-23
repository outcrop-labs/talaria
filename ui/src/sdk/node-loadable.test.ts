// In dev, the app build probe (server/app-health.ts) loads an app's app.ts in
// Node through Vite's SSR loader, so everything @talaria/sdk imports at module
// load has to load without a browser. The router does not: sv-router reads
// `location` and builds an IntersectionObserver when it loads, and once the
// SDK reached it (ViewHeader → lib/view-title, and Markdown → AgentMediaImage
// → SaveDialog) every app's dev probe failed with "location is not defined"
// and the app was disabled. This walks the SDK's static import graph (dynamic
// `import()` is lazy, so it is not followed) and names the chain to any
// browser-only module it reaches.
import { existsSync, readFileSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const SRC = resolve(process.cwd(), 'src')
const ENTRY = join(SRC, 'sdk/index.ts')
const BROWSER_ONLY_FILES = new Set([join(SRC, 'router.ts')])
const BROWSER_ONLY_PACKAGES = ['sv-router']

function stripComments(code: string): string {
  return code
    .replace(/<!--[\s\S]*?-->/g, '')
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:'"`\\])\/\/.*$/gm, '$1')
}

/** Specifiers a module imports at load: static imports and re-exports,
 *  side-effect imports included, `import type` / `export type` left out. */
function loadTimeImports(code: string): string[] {
  const src = stripComments(code)
  const out: string[] = []
  for (const m of src.matchAll(/\b(import|export)\s+(type\s+)?([^;'"()]*?)\s*from\s*['"]([^'"]+)['"]/g)) {
    if (!m[2] && m[4]) out.push(m[4])
  }
  for (const m of src.matchAll(/\bimport\s*['"]([^'"]+)['"]/g)) if (m[1]) out.push(m[1])
  return out
}

function resolveLocal(spec: string, from: string): string | null {
  const base = spec.startsWith('@/')
    ? join(SRC, spec.slice(2))
    : spec.startsWith('.')
      ? resolve(dirname(from), spec)
      : null
  if (!base) return null
  for (const candidate of [base, `${base}.ts`, `${base}.js`, join(base, 'index.ts')]) {
    if (existsSync(candidate) && statSync(candidate).isFile()) return candidate
  }
  return null
}

function sdkGraph(): { files: Set<string>; reaches: Array<{ target: string; chain: string[] }> } {
  const parent = new Map<string, string | null>([[ENTRY, null]])
  const queue = [ENTRY]
  const reaches: Array<{ target: string; chain: string[] }> = []
  const chainTo = (file: string): string[] => {
    const chain: string[] = []
    for (let f: string | null | undefined = file; f; f = parent.get(f)) chain.unshift(relative(SRC, f))
    return chain
  }
  while (queue.length) {
    const file = queue.shift()!
    for (const spec of loadTimeImports(readFileSync(file, 'utf8'))) {
      if (BROWSER_ONLY_PACKAGES.some((p) => spec === p || spec.startsWith(`${p}/`))) {
        reaches.push({ target: spec, chain: chainTo(file) })
        continue
      }
      const next = resolveLocal(spec, file)
      if (!next) continue
      if (BROWSER_ONLY_FILES.has(next)) {
        reaches.push({ target: relative(SRC, next), chain: chainTo(file) })
        continue
      }
      if (parent.has(next)) continue
      parent.set(next, file)
      queue.push(next)
    }
  }
  return { files: new Set(parent.keys()), reaches }
}

describe('@talaria/sdk loads without a browser', () => {
  it('reaches no browser-only module at load', () => {
    const { files, reaches } = sdkGraph()
    // The walk is real: it found the kit the SDK re-exports.
    expect(files.has(join(SRC, 'components/ui/ViewHeader.svelte'))).toBe(true)
    expect(files.has(join(SRC, 'components/ui/Markdown.svelte'))).toBe(true)
    expect(files.size).toBeGreaterThan(40)
    expect(reaches.map((r) => `${[...r.chain, r.target].join(' → ')}`)).toEqual([])
  })
})

describe('loadTimeImports', () => {
  it('reads static imports and re-exports, not type-only or dynamic ones', () => {
    const code = [
      "import { route } from '@/router'",
      "import type { Foo } from './types'",
      "export type { Bar } from './bar'",
      "export { default as X } from './X.svelte'",
      "import './side-effect'",
      "const lazy = () => import('./Lazy.svelte')",
      "// import { nope } from '@/commented'",
      'import {',
      '  a,',
      '  b,',
      "} from '@/lib/multi'",
    ].join('\n')
    expect(loadTimeImports(code).sort()).toEqual(['./X.svelte', './side-effect', '@/lib/multi', '@/router'].sort())
  })
})
