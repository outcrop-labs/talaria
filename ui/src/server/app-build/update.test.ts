// The host's half of an app update, on real trees: the new version is built
// and probed beside the live app without touching it, the app is held from
// every other build while the api swaps the trees, and the switch either
// lands or puts the previous build back.
import { existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterAll, beforeAll, beforeEach, describe, expect, it } from 'vitest'
import { loadAppServer, setSsrInvalidator, setSsrLoader } from '../app-load'
import { buildApp } from './build'
import { appBuildKeyDir, appDir, appNextDir } from './paths'
import { reconcileApp } from './reconcile'
import { readCurrent } from './status'
import { activateUpdate, isHeld, release, stageUpdate } from './update'

const MANIFEST = JSON.stringify({ name: 'Notes', version: '0.1.0' })
const server = (body: string) => `export default { fetch: async () => new Response(${JSON.stringify(body)}) }\n`

function write(dir: string, files: Record<string, string>): void {
  rmSync(dir, { recursive: true, force: true })
  mkdirSync(dir, { recursive: true })
  for (const [name, body] of Object.entries(files)) writeFileSync(join(dir, name), body)
}

/** What the api does between stage and activate: the two trees trade places
 *  (talaria-apps `exchange`; three renames are fine in a test). */
function swap(a: string, b: string): void {
  const tmp = `${a}.swap`
  renameSync(a, tmp)
  renameSync(b, a)
  renameSync(tmp, b)
}

async function served(slug: string): Promise<string> {
  const mod = (await loadAppServer(slug)) as { default: { fetch: () => Promise<Response> } }
  return (await mod.default.fetch()).text()
}

describe('an app update, the host’s half', () => {
  let root = ''
  let generation = 0
  const prev = {
    apps: process.env.TALARIA_APPS_DIR,
    builds: process.env.TALARIA_APP_BUILDS_DIR,
    data: process.env.TALARIA_APP_DATA_DIR,
    api: process.env.TALARIA_RUST_API_URL,
  }

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), 'app-update-'))
    process.env.TALARIA_APPS_DIR = join(root, 'apps')
    process.env.TALARIA_APP_BUILDS_DIR = join(root, 'app-builds')
    process.env.TALARIA_APP_DATA_DIR = join(root, 'app-data')
    process.env.TALARIA_RUST_API_URL = 'off'
    // Dev's loader shape, from source; the invalidator moves the generation
    // so a reload after the swap reads the new file, as Vite's graph does.
    setSsrLoader((abs) => import(/* @vite-ignore */ `${abs}?v=${generation}`))
    setSsrInvalidator(() => {
      generation++
    })
  })

  afterAll(() => {
    setSsrLoader(null)
    setSsrInvalidator(null)
    for (const [k, v] of [
      ['TALARIA_APPS_DIR', prev.apps],
      ['TALARIA_APP_BUILDS_DIR', prev.builds],
      ['TALARIA_APP_DATA_DIR', prev.data],
      ['TALARIA_RUST_API_URL', prev.api],
    ] as const) {
      if (v === undefined) delete process.env[k]
      else process.env[k] = v
    }
    rmSync(root, { recursive: true, force: true })
  })

  beforeEach(async () => {
    release('notes')
    write(appDir('notes'), { 'talaria.json': MANIFEST, 'server.ts': server('v1') })
    rmSync(appNextDir('notes'), { recursive: true, force: true })
    generation++
    const cur = await buildApp('notes')
    expect(cur.status).toBe('ready')
  })

  it('builds and probes the new version beside the live one, holds the app, then switches', async () => {
    const live = readCurrent('notes')!
    write(appNextDir('notes'), { 'talaria.json': MANIFEST, 'server.ts': server('v2') })
    const staged = await stageUpdate('notes')
    expect(staged).toMatchObject({ ok: true })
    if (!staged.ok) return
    expect(staged.build.key).not.toBe(live.key)
    // The live app is untouched and still serves the old version.
    expect(readCurrent('notes')?.key).toBe(live.key)
    expect(await served('notes')).toBe('v1')
    // Held: a rebuild (the watcher's, Rebuild's) leaves it alone.
    expect(isHeld('notes')).toBe(true)
    await reconcileApp('notes')
    expect(readCurrent('notes')?.key).toBe(live.key)

    swap(appDir('notes'), appNextDir('notes'))
    const act = await activateUpdate('notes')
    expect(act).toMatchObject({ ok: true })
    expect(isHeld('notes')).toBe(false)
    expect(readCurrent('notes')?.key).toBe(staged.build.key)
    expect(await served('notes')).toBe('v2')
  }, 120_000)

  it('stops at stage when the new version is refused, and never holds or touches the live app', async () => {
    const live = readCurrent('notes')!
    write(appNextDir('notes'), { 'talaria.json': MANIFEST, 'server.ts': 'export default {\n' })
    const broken = await stageUpdate('notes')
    expect(broken.ok).toBe(false)
    expect(isHeld('notes')).toBe(false)
    expect(readCurrent('notes')).toEqual(live)
    // Its build will never be live: it does not stay on disk.
    if (!broken.ok && broken.build) expect(existsSync(appBuildKeyDir('notes', broken.build.key))).toBe(false)
    expect(existsSync(appBuildKeyDir('notes', live.key))).toBe(true)

    write(appNextDir('notes'), { 'talaria.json': JSON.stringify({ name: 'Notes', surfaces: { dashboard: 'x' } }), 'server.ts': server('v3') })
    const refused = await stageUpdate('notes')
    expect(refused).toMatchObject({ ok: false, error: '"surfaces" names an unknown surface "dashboard" (expected work, manage or settings)' })
    expect(readCurrent('notes')).toEqual(live)

    write(appNextDir('notes'), { 'talaria.json': JSON.stringify({ name: 'Notes', slug: 'other' }), 'server.ts': server('v3') })
    const renamed = await stageUpdate('notes')
    expect(renamed).toMatchObject({ ok: false, error: 'talaria.json declares the slug "other", but the app is installed as "notes"' })
    expect(await served('notes')).toBe('v1')
  }, 120_000)

  it('puts the previous build back when the switched-in version fails its live probe', async () => {
    const live = readCurrent('notes')!
    // Loads where it was staged, throws where it is live.
    write(appNextDir('notes'), {
      'talaria.json': MANIFEST,
      'server.ts': `if (!import.meta.url.includes('.notes.next')) throw new Error('only staged')\n${server('v4')}`,
    })
    expect(await stageUpdate('notes')).toMatchObject({ ok: true })
    swap(appDir('notes'), appNextDir('notes'))
    const act = await activateUpdate('notes')
    expect(act.ok).toBe(false)
    if (!act.ok) expect(act.error).toContain('only staged')
    expect(readCurrent('notes')?.key).toBe(live.key)
    expect(isHeld('notes')).toBe(false)
    if (!act.ok && act.build) expect(existsSync(appBuildKeyDir('notes', act.build.key))).toBe(false)
  }, 120_000)

  it('refuses to activate what was not staged', async () => {
    expect(await activateUpdate('notes')).toMatchObject({ ok: false })
    expect(await stageUpdate('notes')).toMatchObject({ ok: false, error: expect.stringContaining('no staged version') })
  })
})
