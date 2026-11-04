import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { compile, compileModule } from 'svelte/compiler'
import { describe, expect, it } from 'vitest'
import * as zod from 'zod'
import { z } from 'zod'
import * as zodV4 from 'zod/v4'
import zodV4Default, { z as zodV4Z } from 'zod/v4'
import {
  bundledShared,
  clientRuntimePath,
  clientSpecifiers,
  isServerSpecifier,
  serverRuntimeFile,
  serverSpecifiers,
  shimFileName,
  shimSource,
  svelteClientSpecifiers,
} from './specifiers'

describe('app-runtime specifiers', () => {
  it('includes svelte internals the compiler injects', () => {
    const svelte = svelteClientSpecifiers()
    expect(svelte).toContain('svelte')
    expect(svelte).toContain('svelte/internal/client')
    expect(svelte).toContain('svelte/internal/disclose-version')
    expect(svelte).not.toContain('svelte/compiler')
    expect(svelte).not.toContain('svelte/server')
  })

  it('names shims stably', () => {
    expect(shimFileName('@talaria/sdk')).toBe('rt-talaria-sdk')
    expect(shimFileName('svelte/internal/client')).toBe('rt-svelte-internal-client')
  })

  it('flags a bundled svelte module id', () => {
    expect(bundledShared(['/repo/ui/node_modules/svelte/src/internal/client/index.js'])).toHaveLength(1)
    expect(bundledShared(['/repo/apps/demo/app.ts'])).toEqual([])

  })

  it('lists the host SDK as a client specifier', () => {
    expect(clientSpecifiers()).toContain('@talaria/sdk')
  })
})

describe('clientRuntimePath', () => {
  it("maps each shared specifier to its own runtime file, never its package root's", () => {
    expect(clientRuntimePath('svelte')).toBe('/runtime/rt-svelte.js')
    expect(clientRuntimePath('svelte/internal/client')).toBe('/runtime/rt-svelte-internal-client.js')
    expect(clientRuntimePath('svelte/internal/disclose-version')).toBe('/runtime/rt-svelte-internal-disclose-version.js')
    expect(clientRuntimePath('@talaria/sdk')).toBe('/runtime/rt-talaria-sdk.js')
    expect(clientRuntimePath('@tanstack/svelte-query')).toBe('/runtime/rt-tanstack-svelte-query.js')
  })

  it('refuses a subpath the host does not share', () => {
    expect(() => clientRuntimePath('@lucide/svelte/icons/check')).toThrow(/"@lucide\/svelte\/icons\/check" is not shared/)
    expect(() => clientRuntimePath('svelte/server')).toThrow(/not shared/)
  })
})

// An app's server-side code shares the SDK's server entry and zod — the copy
// that entry re-exports as `z` (release sweep apps-2: main's SDK shared zod,
// and apps written then import it).
describe('server specifiers', () => {
  it('share zod beside the SDK, each with its own runtime file', () => {
    expect(serverSpecifiers()).toEqual(['@talaria/sdk/server', 'zod'])
    expect(serverRuntimeFile('@talaria/sdk/server')).toBe('rt-talaria-sdk-server.js')
    expect(serverRuntimeFile('zod')).toBe('rt-zod.js')
  })

  it('share zod/v4 as the same runtime file, and no other subpath, naming the import', () => {
    expect(serverRuntimeFile('zod/v4')).toBe('rt-zod.js')
    expect(isServerSpecifier('zod/v4')).toBe(true)
    // Exact, not a prefix: zod/mini is a different API and has no file.
    expect(isServerSpecifier('zod/mini')).toBe(false)
    expect(isServerSpecifier('zod/v4/core')).toBe(false)
    expect(() => serverRuntimeFile('zod/mini')).toThrow('"zod/mini" is not shared with apps; the host shares "zod" and "zod/v4" only')
    expect(() => serverRuntimeFile('zod/v4/core')).toThrow('the host shares "zod" and "zod/v4" only')
    expect(() => serverRuntimeFile('lodash')).toThrow('"lodash" is not shared with apps')
  })

  it('flag a bundled zod as a shared module leaked into an app chunk', () => {
    expect(bundledShared(['/repo/ui/node_modules/zod/v4/classic/schemas.js'])).toHaveLength(1)
    expect(bundledShared(['/repo/apps/demo/lib/schema.ts'])).toEqual([])
  })
})

describe('shimSource', () => {
  it('re-exports a module whole, as the host build always has', () => {
    expect(shimSource('svelte')).toBe('export * from "svelte";\nimport "svelte";\n')
    expect(shimSource('@talaria/sdk/server', '/ui/src/sdk/server.ts')).toBe('export * from "/ui/src/sdk/server.ts";\nimport "/ui/src/sdk/server.ts";\n')
  })

  it('aliases zod/v4 to the same module: every export of it is the very value zod exports', () => {
    // The alias links zod/v4 to rt-zod.js; this is why that is safe (and why
    // `instanceof ZodError` holds across the two names): there is one zod 4.
    expect(zodV4Z).toBe(z)
    expect(zodV4Default).toBe(z)
    for (const [name, value] of Object.entries(zodV4)) expect(zod[name as keyof typeof zod], name).toBe(value)
  })

  it("carries zod's default too: `import z from 'zod'` is the same namespace as `{ z }`", async () => {
    const dir = mkdtempSync(join(tmpdir(), 'rt-zod-'))
    try {
      mkdirSync(join(dir, 'node_modules'))
      symlinkSync(resolve('node_modules/zod'), join(dir, 'node_modules', 'zod'))
      writeFileSync(join(dir, 'rt-zod.js'), shimSource('zod'))
      const shim = (await import(/* @vite-ignore */ pathToFileURL(join(dir, 'rt-zod.js')).href)) as Record<string, unknown>
      expect(shim.default).toBe(z)
      expect(shim.z).toBe(z)
      // Every export of zod, the very same value: one module, not a copy.
      for (const [name, value] of Object.entries(zod)) expect(shim[name], name).toBe(value)
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})

// Dev loads an app's server.ts, mcp.ts and harnesses/ from source, from apps/
// outside ui/. Vite decides once, for every importer, that a bare `zod` is
// external — the host's own code imports it long before any app — and then
// node-resolves it from the importer's directory: from apps/ that found
// nothing ("Cannot find module 'zod'", and an update adding one rolled back).
// The dev config dedupes it, so it resolves from the root for an app too, to
// the module the host imports. Here: the dev config's own `resolve`, a root
// whose node_modules holds zod, a host module that imports zod first, then an
// app's beside the root.
describe("the dev server resolves an app's server-side zod to the host's", () => {
  it('gives an app file imported after the host the same zod', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'dev-zod-'))
    const { createServer } = await import('vite')
    const devConfig = (await import('../../vite.config')).default as { resolve: { dedupe: string[] } }
    const root = join(dir, 'ui')
    mkdirSync(join(root, 'node_modules'), { recursive: true })
    symlinkSync(resolve('node_modules/zod'), join(root, 'node_modules', 'zod'))
    writeFileSync(join(root, 'host.ts'), "export { z } from 'zod'\n")
    mkdirSync(join(dir, 'apps', 'notes'), { recursive: true })
    writeFileSync(join(dir, 'apps', 'notes', 'server.ts'), "import { z } from 'zod'\nimport zdef from 'zod'\nimport { z as z4 } from 'zod/v4'\nexport { z, zdef, z4 }\n")
    const server = await createServer({
      configFile: false,
      root,
      logLevel: 'silent',
      appType: 'custom',
      resolve: { dedupe: devConfig.resolve.dedupe },
      optimizeDeps: { noDiscovery: true, entries: [] },
      server: { middlewareMode: true, hmr: false, watch: null },
    })
    try {
      const host = (await server.ssrLoadModule('/host.ts')) as { z: unknown }
      const app = (await server.ssrLoadModule(`/@fs${join(dir, 'apps', 'notes', 'server.ts')}`)) as { z: unknown; zdef: unknown; z4: unknown }
      expect(app.z).toBe(host.z)
      expect(app.zdef).toBe(host.z)
      expect(app.z4).toBe(host.z)
      expect(devConfig.resolve.dedupe).toContain('zod')
    } finally {
      await server.close()
      rmSync(dir, { recursive: true, force: true })
    }
  }, 60_000)
})

// The host's runtime/rt-<spec>.js entry is `export * from '<spec>'` built for
// the browser (vite-plugin.ts), so its exports are the spec's browser entry's.
function svelteBrowserEntry(spec: string): string {
  const dir = join(process.cwd(), 'node_modules/svelte')
  const pkg = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8')) as {
    exports: Record<string, string | { browser?: string; default?: string }>
  }
  const entry = pkg.exports[spec === 'svelte' ? '.' : `./${spec.slice('svelte/'.length)}`]
  const rel = typeof entry === 'string' ? entry : (entry?.browser ?? entry?.default)
  if (!rel) throw new Error(`no browser entry for ${spec}`)
  return join(dir, rel)
}

async function runtimeExports(runtimePath: string): Promise<Set<string>> {
  const spec = clientSpecifiers().find((s) => clientRuntimePath(s) === runtimePath)
  if (!spec) throw new Error(`the host builds no ${runtimePath}`)
  const mod = (await import(pathToFileURL(svelteBrowserEntry(spec)).href)) as Record<string, unknown>
  return new Set(Object.keys(mod).filter((k) => k !== 'default'))
}

/** Every name compiled code takes from each import, keyed by the runtime
 *  file the app build rewrites that import to. */
function importedNames(code: string): Map<string, Set<string>> {
  const out = new Map<string, Set<string>>()
  const add = (spec: string, names: string[]) => {
    const path = clientRuntimePath(spec)
    const set = out.get(path) ?? new Set<string>()
    for (const n of names) set.add(n)
    out.set(path, set)
  }
  for (const m of code.matchAll(/^import \* as ([\w$]+) from '([^']+)';$/gm)) {
    const ns = m[1]!.replace(/\$/g, '\\$')
    add(m[2]!, [...code.matchAll(new RegExp(`(?<![\\w$.])${ns}\\.([A-Za-z_$][\\w$]*)`, 'g'))].map((x) => x[1]!))
  }
  for (const m of code.matchAll(/^import \{([^}]*)\} from '([^']+)';$/gm)) {
    add(m[2]!, m[1]!.split(',').map((s) => s.trim().split(/\s+as\s+/)[0]!).filter(Boolean))
  }
  for (const m of code.matchAll(/^import '([^']+)';$/gm)) add(m[1]!, [])
  return out
}

const RUNES_COMPONENT = `<script lang="ts">
  import { onMount, tick, untrack, getContext, setContext } from 'svelte'
  import { fade, fly } from 'svelte/transition'
  import { flip } from 'svelte/animate'
  import { SvelteMap } from 'svelte/reactivity'
  import type { Snippet } from 'svelte'
  let { items = [], value = $bindable(''), onpick, children, row }: {
    items?: string[]; value?: string; onpick?: (s: string) => void; children?: Snippet; row?: Snippet<[string]>
  } = $props()
  let el: HTMLDivElement | undefined = $state()
  let open = $state(false)
  let seen = new SvelteMap<string, number>()
  let shown = $derived(items.filter((i) => i.includes(value)))
  const total = $derived.by(() => shown.length)
  setContext('k', 1)
  const ctx = getContext('k')
  $effect(() => { untrack(() => seen.set(value, shown.length)) })
  onMount(async () => { await tick() })
  function grow(node: HTMLElement) { return { destroy() {} } }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && (open = false)} />
<div bind:this={el} class:open style:--n={total} use:grow {@attach (n) => () => n}>
  <input bind:value placeholder="Filter" />
  {#if open}
    <ul>
      {#each shown as item, i (item)}
        {@const label = \`\${i}: \${item}\`}
        <li animate:flip transition:fly={{ y: 4 }} onclick={() => onpick?.(item)}>
          {#if row}{@render row(item)}{:else}{label}{/if}
        </li>
      {:else}
        <li>None</li>
      {/each}
    </ul>
  {:else}
    <button type="button" onclick={() => (open = true)} {...{ title: 'open' }}>Open {total}</button>
  {/if}
  {#await Promise.resolve(ctx)}<span>…</span>{:then v}<span in:fade>{v}</span>{:catch}<span>!</span>{/await}
  {#key value}<svelte:element this="p">{@html value}</svelte:element>{/key}
  <svelte:boundary>{@render children?.()}{#snippet failed()}<p>failed</p>{/snippet}</svelte:boundary>
  <select bind:value>{#each items as o}<option value={o}>{o}</option>{/each}</select>
  <textarea bind:value></textarea>
  <input type="checkbox" bind:checked={open} />
</div>
`

const LEGACY_COMPONENT = `<script>
  import { createEventDispatcher, onDestroy } from 'svelte'
  import { writable } from 'svelte/store'
  export let count = 0
  const dispatch = createEventDispatcher()
  const store = writable(1)
  $: doubled = count * 2
  onDestroy(() => {})
</script>
<button on:click={() => { count += 1; dispatch('bump', count) }}>{doubled} {$store}</button>
<slot name="extra" />
`

const RUNES_MODULE = `
import { SvelteSet } from 'svelte/reactivity'
export const tags = new SvelteSet<string>()
export const counter = $state({ n: 0 })
export const twice = () => counter.n * 2
export function watch() { return $effect.root(() => { $effect(() => { void counter.n }) }) }
`

describe("a compiled app component's imports resolve against the host runtime", () => {
  const cases: Array<[string, string]> = [
    ['runes component', compile(RUNES_COMPONENT, { generate: 'client', filename: 'Work.svelte', runes: true }).js.code],
    ['legacy component', compile(LEGACY_COMPONENT, { generate: 'client', filename: 'Legacy.svelte' }).js.code],
    ['runes module', compileModule(RUNES_MODULE.replace(/<string>/g, ''), { generate: 'client', filename: 'state.svelte.js' }).js.code],
  ]

  it.each(cases)('%s', async (_name, code) => {
    const imports = importedNames(code)
    // The compiler's own runtime import is always there, on its own shim.
    expect(imports.has('/runtime/rt-svelte-internal-client.js')).toBe(true)
    const missing: string[] = []
    for (const [path, names] of imports) {
      const exported = await runtimeExports(path)
      for (const n of names) if (!exported.has(n)) missing.push(`${path}: ${n}`)
    }
    expect(missing).toEqual([])
  })
})
