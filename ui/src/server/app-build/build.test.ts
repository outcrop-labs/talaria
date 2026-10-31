// A real app build, end to end: the shape the host loads in production.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { clientRuntimePath, clientSpecifiers } from '@/app-runtime/specifiers'
import { clientArtifactOk, probeLoad } from '../app-health'
import { checkAppHarnessesOf, listAppHarnesses } from '../app-harnesses'
import { loadAppHarnesses, setSsrLoader } from '../app-load'
import { buildApp, setServerShim, unsharedImport } from './build'
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


// A HARNESS FILE THAT DOES NOT COMPILE COSTS ITSELF, never the app. A failed
// app build is a disabled app (reconcile → failHealth), and a harness is
// something the app ships for model fitness, not something it needs to run.
// The server-side builds link `@talaria/sdk/server` to the host build's shim;
// with no host build here, a stand-in shim takes its place.
describe('buildApp (harnesses)', () => {
  let root = ''
  const prev = { apps: process.env.TALARIA_APPS_DIR, builds: process.env.TALARIA_APP_BUILDS_DIR }
  const manifest = JSON.stringify({ name: 'Harness Check', icon: '⬡', version: '0.1.0' })
  const server = 'export default { fetch: async () => new Response("ok") }\n'
  const good = `import { defineHarness } from '@talaria/sdk/server'
export default defineHarness({
  id: 'check:good',
  label: 'Good',
  job: 'A harness that compiles.',
  requires: [],
  floor: { capabilities: [], refuseBelow: false, note: 'Runs on anything.' },
  model: {},
  render: (input: string) => [{ role: 'user', content: input }],
  output: { kind: 'text' },
  onFailure: 'null',
  evals: [{ name: 'says hi', input: 'hi', check: () => null }],
})
`
  const app = (slug: string, files: Record<string, string>) => {
    const dir = join(root, 'apps', slug)
    mkdirSync(join(dir, 'harnesses'), { recursive: true })
    for (const [name, body] of Object.entries(files)) writeFileSync(join(dir, name), body)
  }
  const only = (slug: string) => ({ enabled: async () => [{ slug, name: slug }], warn: () => {} })

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'app-build-harnesses-'))
    process.env.TALARIA_APPS_DIR = join(root, 'apps')
    process.env.TALARIA_APP_BUILDS_DIR = join(root, 'app-builds')
    const shim = join(root, 'rt-talaria-sdk-server.js')
    writeFileSync(shim, 'export const defineHarness = (h) => h\n')
    setServerShim(shim)
    app('mixed', {
      'talaria.json': manifest,
      'server.ts': server,
      'harnesses/good.ts': good,
      'harnesses/broken.ts': 'export default {\n',
      'harnesses/unresolved.ts': "import missing from './not-there'\nexport default missing\n",
    })
    app('all-broken', { 'talaria.json': manifest, 'server.ts': server, 'harnesses/broken.ts': 'export default {\n' })
  })

  afterAll(() => {
    setServerShim(undefined)
    setSsrLoader(null)
    if (prev.apps === undefined) delete process.env.TALARIA_APPS_DIR
    else process.env.TALARIA_APPS_DIR = prev.apps
    if (prev.builds === undefined) delete process.env.TALARIA_APP_BUILDS_DIR
    else process.env.TALARIA_APP_BUILDS_DIR = prev.builds
    rmSync(root, { recursive: true, force: true })
  })

  it('builds an app with one good and two broken harness files ready; the good one registers, each broken one is reported', async () => {
    const cur = await buildApp('mixed')
    expect(cur.error ?? null).toBeNull()
    expect(cur.status).toBe('ready')
    expect(cur.files).toEqual(['server.js', 'harnesses.js'])
    const out = appBuildKeyDir('mixed', cur.key)
    // The per-file checks leave nothing behind.
    expect(existsSync(join(out, '.harness-check'))).toBe(false)
    expect(existsSync(join(out, '.harnesses-entry.js'))).toBe(false)

    setSsrLoader(null)
    const files = await loadAppHarnesses('mixed')
    expect(files.map((f) => [f.file, f.compileError !== undefined])).toEqual([
      ['broken.ts', true],
      ['good.ts', false],
      ['unresolved.ts', true],
    ])

    const listing = await listAppHarnesses(only('mixed'))
    expect(listing.harnesses.map((h) => h.def.id)).toEqual(['check:good'])
    expect(listing.problems.map((p) => p.file)).toEqual(['broken.ts', 'unresolved.ts'])
    for (const p of listing.problems) expect(p.reason).toMatch(/^it failed to compile: /)
    // The compiler's sentence, one line, naming the file by its path in the
    // app — no code frame, no absolute or host-relative path.
    expect(listing.problems[0]!.reason).toMatch(/^it failed to compile: .*harnesses\/broken\.ts:2:0: ERROR: /)
    expect(listing.problems[1]!.reason).toContain('Could not resolve "./not-there" from "harnesses/unresolved.ts"')
    for (const p of listing.problems) {
      expect(p.reason).not.toContain(root)
      expect(p.reason).not.toContain('../')
      expect(p.reason).not.toContain('|')
    }

    // What the load probe writes on the build status, where Manage → Apps reads it.
    const report = await checkAppHarnessesOf('mixed', only('mixed'))
    expect(report.registered).toEqual(['check:good'])
    expect(report.problems.map((p) => p.split(':')[0])).toEqual(['harnesses/broken.ts', 'harnesses/unresolved.ts'])
  }, 180_000)

  it('still builds ready when no harness file compiles — with no harness, and each file named', async () => {
    const cur = await buildApp('all-broken')
    expect(cur.error ?? null).toBeNull()
    expect(cur.status).toBe('ready')
    expect(cur.files).toContain('harnesses.js')
    setSsrLoader(null)
    const listing = await listAppHarnesses(only('all-broken'))
    expect(listing.harnesses).toEqual([])
    expect(listing.problems.map((p) => [p.file, p.reason.startsWith('it failed to compile: ')])).toEqual([['broken.ts', true]])
  }, 180_000)
})

// An app has no node_modules where it is installed, so only what the host
// shares resolves. The recipe says so by name — the same sentence on the host
// and in the SDK kit's sandbox, where a tree CAN sit under a node_modules that
// holds the package and would otherwise bundle a copy and pass.
describe('unsharedImport', () => {
  it('lets paths, schemes and the shared specifiers through, per kind', () => {
    for (const ok of ['./Work.svelte', '../x', '/abs/path.ts', '\0vite/preload-helper.js', 'node:fs', 'data:text/plain,x']) {
      expect(unsharedImport(ok, 'client'), ok).toBeNull()
      expect(unsharedImport(ok, 'server'), ok).toBeNull()
    }
    for (const ok of ['svelte', 'svelte/transition', '@talaria/sdk', '@lucide/svelte', '@tanstack/svelte-query']) expect(unsharedImport(ok, 'client'), ok).toBeNull()
    for (const ok of ['@talaria/sdk/server', 'fs', 'fs/promises', 'crypto']) expect(unsharedImport(ok, 'server'), ok).toBeNull()
  })

  it('names a package the host does not share', () => {
    expect(unsharedImport('zod', 'server')).toBe(
      '"zod" is not shared with apps: an app has no node_modules of its own where it is installed, so a server-side module may import only @talaria/sdk/server (which re-exports zod as z) and node\'s builtins',
    )
    expect(unsharedImport('lodash', 'client')).toContain('so a surface may import only svelte, @talaria/sdk')
    // The client SDK is not the server's, nor the test entry anyone's.
    expect(unsharedImport('@talaria/sdk', 'server')).toContain('"@talaria/sdk" is not shared')
    expect(unsharedImport('@talaria/sdk/testing', 'server')).toContain('"@talaria/sdk/testing" is not shared')
    expect(unsharedImport('fs', 'client')).toContain('"fs" is not shared')
  })
})

describe('buildApp (a package the app does not get)', () => {
  let root = ''
  const prev = { apps: process.env.TALARIA_APPS_DIR, builds: process.env.TALARIA_APP_BUILDS_DIR }
  const manifest = JSON.stringify({ name: 'Import Check', icon: '⬡', version: '0.1.0' })

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'app-build-imports-'))
    process.env.TALARIA_APPS_DIR = join(root, 'apps')
    process.env.TALARIA_APP_BUILDS_DIR = join(root, 'app-builds')
    const shim = join(root, 'rt-talaria-sdk-server.js')
    writeFileSync(shim, 'export const defineHarness = (h) => h\nexport const z = {}\n')
    setServerShim(shim)
    // zod IS resolvable from the app's tree: a node_modules above it holds it.
    mkdirSync(join(root, 'node_modules'), { recursive: true })
    symlinkSync(resolve('node_modules/zod'), join(root, 'node_modules', 'zod'))
    const dir = join(root, 'apps', 'imports-zod')
    mkdirSync(join(dir, 'harnesses'), { recursive: true })
    writeFileSync(join(dir, 'talaria.json'), manifest)
    writeFileSync(join(dir, 'mcp.ts'), "import { z } from 'zod'\nexport default { tools: [{ name: 'x', description: String(z), handler: () => 1 }] }\n")
    const harnessDir = join(root, 'apps', 'harness-zod')
    mkdirSync(join(harnessDir, 'harnesses'), { recursive: true })
    writeFileSync(join(harnessDir, 'talaria.json'), manifest)
    writeFileSync(join(harnessDir, 'server.ts'), 'export default { fetch: async () => new Response("ok") }\n')
    writeFileSync(join(harnessDir, 'harnesses', 'z.ts'), "import { z } from 'zod'\nexport default { id: String(z) }\n")
  })

  afterAll(() => {
    setServerShim(undefined)
    if (prev.apps === undefined) delete process.env.TALARIA_APPS_DIR
    else process.env.TALARIA_APPS_DIR = prev.apps
    if (prev.builds === undefined) delete process.env.TALARIA_APP_BUILDS_DIR
    else process.env.TALARIA_APP_BUILDS_DIR = prev.builds
    rmSync(root, { recursive: true, force: true })
  })

  it('refuses a server-side import of zod even where zod resolves, saying where z comes from', async () => {
    const cur = await buildApp('imports-zod')
    expect(cur.status).toBe('failed')
    expect(cur.error).toContain('"zod" is not shared with apps')
    expect(cur.error).toContain('@talaria/sdk/server (which re-exports zod as z)')
  }, 180_000)

  it('skips a harness file that imports zod, with the same sentence, and still builds the app', async () => {
    const cur = await buildApp('harness-zod')
    expect(cur.status).toBe('ready')
    setSsrLoader(null)
    const files = await loadAppHarnesses('harness-zod')
    expect(files.map((f) => f.file)).toEqual(['z.ts'])
    expect(files[0]!.compileError).toContain('"zod" is not shared with apps')
  }, 180_000)
})
