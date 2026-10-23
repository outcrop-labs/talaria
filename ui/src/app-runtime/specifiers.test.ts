import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { compile, compileModule } from 'svelte/compiler'
import { describe, expect, it } from 'vitest'
import { bundledShared, clientRuntimePath, clientSpecifiers, shimFileName, svelteClientSpecifiers } from './specifiers'

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
