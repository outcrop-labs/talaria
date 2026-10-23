// A real app build, end to end: the shape the host loads in production.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { clientRuntimePath, clientSpecifiers } from '@/app-runtime/specifiers'
import { clientArtifactOk, probeLoad } from '../app-health'
import { setSsrLoader } from '../app-load'
import { buildApp } from './build'
import { BUILD_RECIPE, sourceKey } from './key'
import { appBuildKeyDir, appSrc } from './paths'
import { readCurrent, writeCurrent } from './status'

const APP = {
  'talaria.json': JSON.stringify({ name: 'Build Check', icon: '⬡', version: '0.1.0', surfaces: { work: 'Build Check' } }),
  'app.ts': `import { defineApp } from '@talaria/sdk'
import Work from './Work.svelte'
export default defineApp({ work: Work })
`,
  'Work.svelte': `<script lang="ts">
  import { Button, EmptyState } from '@talaria/sdk'
  import { fade } from 'svelte/transition'
  let items = $state<string[]>([])
  const add = () => { items = [...items, String(items.length + 1)] }
</script>
<div class="p-4">
  <Button size="sm" onclick={add}>Add</Button>
  {#each items as item (item)}<p transition:fade>{item}</p>{:else}<EmptyState title="Nothing yet" />{/each}
</div>
`,
}

describe('buildApp (client)', () => {
  let root = ''
  const prev = { apps: process.env.TALARIA_APPS_DIR, builds: process.env.TALARIA_APP_BUILDS_DIR }

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'app-build-'))
    process.env.TALARIA_APPS_DIR = join(root, 'apps')
    process.env.TALARIA_APP_BUILDS_DIR = join(root, 'app-builds')
    const dir = join(root, 'apps', 'build-check')
    mkdirSync(dir, { recursive: true })
    for (const [name, body] of Object.entries(APP)) writeFileSync(join(dir, name), body)
  })

  afterAll(() => {
    if (prev.apps === undefined) delete process.env.TALARIA_APPS_DIR
    else process.env.TALARIA_APPS_DIR = prev.apps
    if (prev.builds === undefined) delete process.env.TALARIA_APP_BUILDS_DIR
    else process.env.TALARIA_APP_BUILDS_DIR = prev.builds
    rmSync(root, { recursive: true, force: true })
  })

  it('emits an app.js that keeps its default export and imports only host runtime files', async () => {
    const cur = await buildApp('build-check')
    expect(cur.error ?? null).toBeNull()
    expect(cur.status).toBe('ready')
    expect(cur.files).toContain('app.js')
    const code = readFileSync(join(appBuildKeyDir('build-check', cur.key), 'app.js'), 'utf8')

    // The host imports app.js and mounts its default export; the prod probe
    // refuses a bundle without one.
    expect(clientArtifactOk(code)).toBe(true)

    const specs = [
      ...[...code.matchAll(/\bfrom\s*["']([^"']+)["']/g)].map((m) => m[1]!),
      ...[...code.matchAll(/\bimport\s*["']([^"']+)["']/g)].map((m) => m[1]!),
    ]
    const runtime = new Set(clientSpecifiers().map(clientRuntimePath))
    expect(specs.length).toBeGreaterThan(0)
    expect(specs.filter((s) => !runtime.has(s))).toEqual([])
    // The compiled component's runtime comes from its own shim.
    expect(specs).toContain('/runtime/rt-svelte-internal-client.js')
    expect(specs).toContain('/runtime/rt-talaria-sdk.js')
    expect(specs).toContain('/runtime/rt-svelte-transition.js')
  }, 180_000)

  it('rebuilds a ready build an older recipe left behind, and reuses one made by this recipe', async () => {
    const first = await buildApp('build-check')
    expect(first.status).toBe('ready')
    const appDir = join(appSrc('build-check', 'talaria.json'), '..')
    expect(first.key).toBe(sourceKey(appDir, first.hostBuild, BUILD_RECIPE))

    // What a build from before the bump looks like: same sources, same host
    // build id (no TALARIA_VERSION), marked ready, and broken (no default export).
    const stale = sourceKey(appDir, first.hostBuild, BUILD_RECIPE - 1)
    expect(stale).not.toBe(first.key)
    mkdirSync(appBuildKeyDir('build-check', stale), { recursive: true })
    writeFileSync(join(appBuildKeyDir('build-check', stale), 'app.js'), 'import"/runtime/rt-svelte.js";\n')
    writeCurrent('build-check', { ...first, key: stale, builtAt: 1, files: ['app.js'] })

    const rebuilt = await buildApp('build-check')
    expect(rebuilt.status).toBe('ready')
    expect(rebuilt.key).toBe(first.key)
    expect(rebuilt.builtAt).toBeGreaterThan(1)
    expect(readCurrent('build-check')?.key).toBe(first.key)
    expect(clientArtifactOk(readFileSync(join(appBuildKeyDir('build-check', rebuilt.key), 'app.js'), 'utf8'))).toBe(true)

    // The cache still works: a ready build under this recipe's key is returned as is.
    writeCurrent('build-check', { ...rebuilt, builtAt: 2 })
    expect((await buildApp('build-check')).builtAt).toBe(2)
  }, 180_000)

  it('fails a build that imports a subpath the host does not share, naming it', async () => {
    const dir = join(root, 'apps', 'deep-import')
    mkdirSync(dir, { recursive: true })
    writeFileSync(join(dir, 'talaria.json'), APP['talaria.json'])
    writeFileSync(join(dir, 'app.ts'), APP['app.ts'])
    writeFileSync(
      join(dir, 'Work.svelte'),
      `<script lang="ts">\n  import Check from '@lucide/svelte/icons/check'\n</script>\n<Check />\n`,
    )
    const cur = await buildApp('deep-import')
    expect(cur.status).toBe('failed')
    expect(cur.error).toContain('"@lucide/svelte/icons/check" is not shared with apps')
  }, 180_000)
})

// An app with no UI: talaria.json declares no surfaces, and there is no
// app.ts. It builds, and the load probe passes it, when it has a server or
// agent tools to run; an app that declares a surface still needs app.ts.
describe('buildApp (no client)', () => {
  let root = ''
  const prev = { apps: process.env.TALARIA_APPS_DIR, builds: process.env.TALARIA_APP_BUILDS_DIR }
  const app = (slug: string, files: Record<string, string>) => {
    const dir = join(root, 'apps', slug)
    mkdirSync(dir, { recursive: true })
    for (const [name, body] of Object.entries(files)) writeFileSync(join(dir, name), body)
  }

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'app-build-clientless-'))
    process.env.TALARIA_APPS_DIR = join(root, 'apps')
    process.env.TALARIA_APP_BUILDS_DIR = join(root, 'app-builds')
    const manifest = JSON.stringify({ name: 'Server Only', icon: '⬡', version: '0.1.0' })
    app('server-only', { 'talaria.json': manifest, 'server.ts': 'export default { fetch: async () => new Response("ok") }\n' })
    app('mcp-only', { 'talaria.json': manifest, 'mcp.ts': 'export default { tools: [] }\n' })
    app('declares-work', {
      'talaria.json': JSON.stringify({ name: 'Needs UI', surfaces: { work: 'Needs UI', settings: 'Needs UI' } }),
      'server.ts': 'export default { fetch: async () => new Response("ok") }\n',
    })
    app('nothing', { 'talaria.json': manifest })
  })

  afterAll(() => {
    setSsrLoader(null)
    if (prev.apps === undefined) delete process.env.TALARIA_APPS_DIR
    else process.env.TALARIA_APPS_DIR = prev.apps
    if (prev.builds === undefined) delete process.env.TALARIA_APP_BUILDS_DIR
    else process.env.TALARIA_APP_BUILDS_DIR = prev.builds
    rmSync(root, { recursive: true, force: true })
  })

  it('builds a server-only and an MCP-only app with no app.js', async () => {
    for (const slug of ['server-only', 'mcp-only']) {
      const cur = await buildApp(slug)
      expect(cur.error ?? null).toBeNull()
      expect(cur.status).toBe('ready')
      expect(cur.files).not.toContain('app.js')
      expect(cur.files).not.toContain('app.css')
    }
  }, 180_000)

  it('the load probe passes them, loading what they ship', async () => {
    // Dev's loader shape: the module from source.
    setSsrLoader((abs) => import(/* @vite-ignore */ abs))
    expect(await probeLoad('server-only')).toEqual({ ok: true })
    expect(await probeLoad('mcp-only')).toEqual({ ok: true })
  }, 60_000)

  it('still requires app.ts of an app that declares a surface, and something to run of every app', async () => {
    const declares = await buildApp('declares-work')
    expect(declares.status).toBe('failed')
    expect(declares.error).toBe('talaria.json declares surfaces (work, settings), so app.ts is required')
    expect(await probeLoad('declares-work')).toEqual({ ok: false, error: 'talaria.json declares surfaces (work, settings), so app.ts is required' })
    const empty = await buildApp('nothing')
    expect(empty.status).toBe('failed')
    expect(empty.error).toBe('an app needs at least one of app.ts, server.ts or mcp.ts')
    expect(await probeLoad('nothing')).toEqual({ ok: false, error: 'an app needs at least one of app.ts, server.ts or mcp.ts' })
  }, 180_000)
})

