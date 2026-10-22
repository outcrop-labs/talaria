// A real app build, end to end: the shape the host loads in production.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { clientRuntimePath, clientSpecifiers } from '@/app-runtime/specifiers'
import { clientArtifactOk } from '../app-health'
import { buildApp } from './build'
import { appBuildKeyDir } from './paths'

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
