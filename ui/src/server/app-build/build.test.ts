// A real app build, end to end: the shape the host loads in production.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { clientRuntimePath, clientSpecifiers, shimSource } from '@/app-runtime/specifiers'
import { clientArtifactOk, probeLoad } from '../app-health'
import { checkAppHarnessesOf, listAppHarnesses } from '../app-harnesses'
import { loadAppHarnesses, loadAppMcp, loadAppServer, setSsrLoader } from '../app-load'
import { buildApp, setServerShims, unsharedImport } from './build'
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
// with no host build here, a stand-in shim in a directory of its own takes
// its place.
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
    writeFileSync(join(root, 'rt-talaria-sdk-server.js'), 'export const defineHarness = (h) => h\n')
    setServerShims(root)
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
    setServerShims(undefined)
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
    for (const ok of ['@talaria/sdk/server', 'zod', 'fs', 'fs/promises', 'crypto']) expect(unsharedImport(ok, 'server'), ok).toBeNull()
  })

  it('names a package the host does not share', () => {
    expect(unsharedImport('clsx', 'server')).toBe(
      '"clsx" is not shared with apps: an app has no node_modules of its own where it is installed, so a server-side module may import only @talaria/sdk/server, zod and node\'s builtins',
    )
    expect(unsharedImport('lodash', 'client')).toContain('so a surface may import only svelte, @talaria/sdk')
    // The client SDK is not the server's, nor the test entry anyone's.
    expect(unsharedImport('@talaria/sdk', 'server')).toContain('"@talaria/sdk" is not shared')
    expect(unsharedImport('@talaria/sdk/testing', 'server')).toContain('"@talaria/sdk/testing" is not shared')
    expect(unsharedImport('fs', 'client')).toContain('"fs" is not shared')
  })

  it('says a server-side package is the server side\'s, and how a surface takes its types', () => {
    expect(unsharedImport('zod', 'client')).toBe(
      '"zod" is shared with server-side modules only (server.ts, mcp.ts, harnesses/): a surface may import only svelte, @talaria/sdk, @lucide/svelte and @tanstack/svelte-query, and takes a type from a module that imports zod by `import type`, which the build erases',
    )
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
    writeFileSync(join(root, 'rt-talaria-sdk-server.js'), 'export const defineHarness = (h) => h\n')
    setServerShims(root)
    // clsx IS resolvable from the app's tree: a node_modules above it holds it.
    mkdirSync(join(root, 'node_modules'), { recursive: true })
    symlinkSync(resolve('node_modules/clsx'), join(root, 'node_modules', 'clsx'))
    const dir = join(root, 'apps', 'imports-clsx')
    mkdirSync(join(dir, 'harnesses'), { recursive: true })
    writeFileSync(join(dir, 'talaria.json'), manifest)
    writeFileSync(join(dir, 'mcp.ts'), "import { clsx } from 'clsx'\nexport default { tools: [{ name: 'x', description: clsx('a'), handler: () => 1 }] }\n")
    const harnessDir = join(root, 'apps', 'harness-clsx')
    mkdirSync(join(harnessDir, 'harnesses'), { recursive: true })
    writeFileSync(join(harnessDir, 'talaria.json'), manifest)
    writeFileSync(join(harnessDir, 'server.ts'), 'export default { fetch: async () => new Response("ok") }\n')
    writeFileSync(join(harnessDir, 'harnesses', 'c.ts'), "import { clsx } from 'clsx'\nexport default { id: clsx('c') }\n")
  })

  afterAll(() => {
    setServerShims(undefined)
    if (prev.apps === undefined) delete process.env.TALARIA_APPS_DIR
    else process.env.TALARIA_APPS_DIR = prev.apps
    if (prev.builds === undefined) delete process.env.TALARIA_APP_BUILDS_DIR
    else process.env.TALARIA_APP_BUILDS_DIR = prev.builds
    rmSync(root, { recursive: true, force: true })
  })

  it('refuses a server-side import of a package the host does not share, even where it resolves', async () => {
    const cur = await buildApp('imports-clsx')
    expect(cur.status).toBe('failed')
    expect(cur.error).toContain('"clsx" is not shared with apps')
    expect(cur.error).toContain("may import only @talaria/sdk/server, zod and node's builtins")
  }, 180_000)

  it('skips a harness file that imports one, with the same sentence, and still builds the app', async () => {
    const cur = await buildApp('harness-clsx')
    expect(cur.status).toBe('ready')
    setSsrLoader(null)
    const files = await loadAppHarnesses('harness-clsx')
    expect(files.map((f) => f.file)).toEqual(['c.ts'])
    expect(files[0]!.compileError).toContain('"clsx" is not shared with apps')
  }, 180_000)
})

// zod IS shared with an app's server-side code, and it is the host's one
// copy: the module `@talaria/sdk/server` re-exports as `z`. main's SDK said
// so, and apps written then import it directly — a lib/schema.ts that
// imports only zod, so the app's pure half runs under a plain `bun test`, and
// that a surface takes types from by `import type`. Refused, every such app
// was turned off at the first boot after an upgrade (release sweep apps-2).
// The stand-in shims here re-export the zod a node_modules above them holds,
// which is ui's own, as the host build's do.
describe('buildApp (zod, shared with server-side code)', () => {
  let root = ''
  let shims = ''
  const prev = { apps: process.env.TALARIA_APPS_DIR, builds: process.env.TALARIA_APP_BUILDS_DIR }
  const files = {
    'talaria.json': JSON.stringify({ name: 'Zod Check', icon: '⬡', version: '0.1.0', surfaces: { work: 'Zod Check' } }),
    'lib/schema.ts': "import { z } from 'zod'\nexport const Note = z.object({ title: z.string().min(1) })\nexport type Note = z.infer<typeof Note>\n",
    'server.ts': `import { z } from 'zod'
import * as zns from 'zod'
import zdef from 'zod'
import { z as sdkZ } from '@talaria/sdk/server'
import { Note } from './lib/schema'
export default { fetch: async () => new Response('ok') }
export const same = {
  named: z === sdkZ,
  namespace: zns.object === sdkZ.object,
  default: zdef === sdkZ,
  instance: Note instanceof sdkZ.ZodObject,
  parsed: Note.safeParse({ title: 'x' }).success,
  refused: Note.safeParse({ title: '' }).success,
}
`,
    'mcp.ts': "import { z } from 'zod'\nconst Input = z.object({ title: z.string() })\nexport default { tools: [{ name: 'echo_title', description: 'Echo a title.', handler: (args: unknown) => Input.parse(args) }] }\n",
    'harnesses/note.ts': "import { defineHarness } from '@talaria/sdk/server'\nimport { Note } from '../lib/schema'\nexport default defineHarness({ id: 'zod:note', parse: (v: unknown) => Note.parse(v) })\n",
    'app.ts': "import { defineApp } from '@talaria/sdk'\nimport Work from './Work.svelte'\nexport default defineApp({ work: Work })\n",
    // A type from the zod-importing module: erased, so the surface builds.
    'Work.svelte': `<script lang="ts">\n  import type { Note } from './lib/schema'\n  const notes: Note[] = []\n</script>\n<p>{notes.length}</p>\n`,
  }
  const app = (slug: string, over: Record<string, string | null> = {}) => {
    const dir = join(root, 'apps', slug)
    for (const [name, body] of Object.entries({ ...files, ...over })) {
      if (body === null) continue
      mkdirSync(join(dir, name, '..'), { recursive: true })
      writeFileSync(join(dir, name), body)
    }
  }

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'app-build-zod-'))
    process.env.TALARIA_APPS_DIR = join(root, 'apps')
    process.env.TALARIA_APP_BUILDS_DIR = join(root, 'app-builds')
    shims = join(root, 'runtime')
    mkdirSync(join(root, 'node_modules'), { recursive: true })
    symlinkSync(resolve('node_modules/zod'), join(root, 'node_modules', 'zod'))
    mkdirSync(shims, { recursive: true })
    writeFileSync(join(shims, 'rt-talaria-sdk-server.js'), "export { z } from 'zod'\nexport const defineHarness = (h) => h\n")
    writeFileSync(join(shims, 'rt-zod.js'), shimSource('zod'))
    setServerShims(shims)
    app('zod-check')
    app('zod-subpath', { 'app.ts': null, 'Work.svelte': null, 'talaria.json': JSON.stringify({ name: 'Zod Subpath', version: '0.1.0' }), 'mcp.ts': "import { z } from 'zod/mini'\nexport default { tools: [{ name: 'x', description: String(z), handler: () => 1 }] }\n" })
    app('zod-surface', { 'Work.svelte': `<script lang="ts">\n  import { Note } from './lib/schema'\n</script>\n<p>{String(Note)}</p>\n` })
  })

  afterAll(() => {
    setServerShims(undefined)
    setSsrLoader(null)
    if (prev.apps === undefined) delete process.env.TALARIA_APPS_DIR
    else process.env.TALARIA_APPS_DIR = prev.apps
    if (prev.builds === undefined) delete process.env.TALARIA_APP_BUILDS_DIR
    else process.env.TALARIA_APP_BUILDS_DIR = prev.builds
    rmSync(root, { recursive: true, force: true })
  })

  it('builds an app whose server, tools and harness import zod, linking it to the host runtime and bundling none', async () => {
    const cur = await buildApp('zod-check')
    expect(cur.error ?? null).toBeNull()
    expect(cur.status).toBe('ready')
    expect(cur.files).toEqual(['app.js', 'app.css', 'server.js', 'mcp.js', 'harnesses.js'])
    const out = appBuildKeyDir('zod-check', cur.key)
    const server = readFileSync(join(out, 'server.js'), 'utf8')
    expect(server).toContain(pathToFileURL(join(shims, 'rt-zod.js')).href)
    expect(server).toContain(pathToFileURL(join(shims, 'rt-talaria-sdk-server.js')).href)
    expect(server).not.toMatch(/from\s*["']zod["']/)
    // Not a copy: zod's own source would carry its classes.
    expect(server).not.toContain('$ZodType')
  }, 180_000)

  it('loads the built server with one zod: z from the SDK and zod itself, named, whole and default, are one module', async () => {
    setSsrLoader(null)
    const mod = (await loadAppServer('zod-check')) as { same?: Record<string, boolean> } | null
    expect(mod?.same).toEqual({ named: true, namespace: true, default: true, instance: true, parsed: true, refused: false })
    const mcp = (await loadAppMcp('zod-check')) as { default: { tools: { handler: (a: unknown) => unknown }[] } }
    expect(mcp.default.tools[0]!.handler({ title: 't' })).toEqual({ title: 't' })
    const harnesses = await loadAppHarnesses('zod-check')
    expect(harnesses.map((h) => [h.file, h.compileError ?? null])).toEqual([['note.ts', null]])
    const note = (await harnesses[0]!.load()) as { default: { id: string; parse: (v: unknown) => unknown } }
    expect(note.default.id).toBe('zod:note')
    expect(note.default.parse({ title: 'a' })).toEqual({ title: 'a' })
  }, 60_000)

  it('refuses a subpath of zod, which the host does not share, naming it', async () => {
    const cur = await buildApp('zod-subpath')
    expect(cur.status).toBe('failed')
    expect(cur.error).toContain('"zod/mini" is not shared with apps; the host shares "zod" and its listed subpaths only')
  }, 180_000)

  it('refuses zod in a surface — a value import from the module that uses it — saying a type comes in by `import type`', async () => {
    const cur = await buildApp('zod-surface')
    expect(cur.status).toBe('failed')
    expect(cur.error).toContain('"zod" is shared with server-side modules only')
    expect(cur.error).toContain('`import type`')
  }, 180_000)
})
