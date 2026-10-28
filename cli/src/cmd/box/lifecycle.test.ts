// A box's app databases (ui/src/server/app-db.ts): the box compose puts them
// on the box's own network, and `box rm` takes them down with the box. The
// template is read as text, because the network name it interpolates and the
// one `box rm` filters on must stay the same string. `box rm` also empties the
// state from a root container, since an app database owns its own files.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { boxAppDbs, runRm } from './lifecycle'
import { boxNetwork, COMPOSE_FILE, IMAGE } from './shared'
import { fakeCtx } from '../../testing'

const makeBox = (name: string): { root: string; box: string } => {
  const root = mkdtempSync(join(tmpdir(), 'talaria-box-'))
  const box = join(root, '..', 'devboxes', name)
  mkdirSync(join(box, 'talaria/.git'), { recursive: true })
  writeFileSync(join(box, 'box.env'), `BOX_NAME=${name}\nPROJECT=devbox-${name}\n`)
  return { root, box }
}

const psArgs = (name: string): string[] => [
  'ps', '-a',
  '--filter', `network=devbox-${name}_default`,
  '--filter', 'name=^talaria-appdb-',
  '--format', '{{.Names}}',
]

describe('a devbox dials its app databases by name on its own network', () => {
  const tpl = readFileSync(join(import.meta.dir, '../../../..', COMPOSE_FILE), 'utf8')

  test('the box compose names the app DB network the box rm filters on', () => {
    expect(tpl).toContain('TALARIA_APP_DB_NETWORK: devbox-${BOX_NAME}_default')
    expect(boxNetwork('demo')).toBe('devbox-demo_default')
  })

  // The instance in `talaria-appdb-<instance>-<slug>`: a fixed '1' named every
  // box's databases alike, so two boxes running one app shared a container.
  test("the box's TALARIA_DEVBOX is its name, not a flag", () => {
    expect(tpl).toContain('TALARIA_DEVBOX: ${BOX_NAME}')
    expect(tpl).not.toContain("TALARIA_DEVBOX: '1'")
  })
})

describe('box rm takes the app databases with it', () => {
  test('lists them by the box network, and removes each before the project goes down', async () => {
    const { root } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', psArgs('demo')], 'talaria-appdb-demo-notes\ntalaria-appdb-demo-crm\n')
    await runRm(ctx, 'demo', true)
    const rms = ctx.calls.filter((c) => c.args[0] === 'rm')
    expect(rms.map((c) => c.args)).toEqual([
      ['rm', '-f', '-v', 'talaria-appdb-demo-notes'],
      ['rm', '-f', '-v', 'talaria-appdb-demo-crm'],
    ])
    // Before the box project's `down`, which cannot remove a network an app
    // database still holds.
    const firstDown = ctx.calls.findIndex((c) => c.args.includes('down'))
    const lastRm = ctx.calls.lastIndexOf(rms.at(-1)!)
    expect(lastRm).toBeLessThan(firstDown)
  })

  test('a box with no app databases removes none', async () => {
    const { root } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    await runRm(ctx, 'demo', true)
    expect(ctx.calls.some((c) => c.args[0] === 'rm')).toBe(false)
    expect(ctx.calls.some((c) => c.args.includes('down'))).toBe(true)
  })

  // An app database's cluster is the postgres image's uid 70, mode 0700: this
  // user cannot delete it, so a root container empties the state first.
  test('empties the state from a root container before the directory goes', async () => {
    const { root, box } = makeBox('demo')
    mkdirSync(join(box, 'state/app-data/notes'), { recursive: true })
    const ctx = fakeCtx()
    ctx.root = root
    await runRm(ctx, 'demo', true)
    const run = ctx.calls.find((c) => c.args[0] === 'run')
    expect(run?.args).toEqual([
      'run', '--rm', '--network', 'none', '--user', '0:0',
      '-v', `${join(box, 'state')}:/state`,
      '--entrypoint', 'find', IMAGE, '/state', '-mindepth', '1', '-delete',
    ])
    // After everything that could still be writing there is down.
    const lastDown = ctx.calls.map((c) => c.args.includes('down')).lastIndexOf(true)
    expect(ctx.calls.indexOf(run!)).toBeGreaterThan(lastDown)
    expect(await Bun.file(join(box, 'box.env')).exists()).toBe(false)
  })

  test('a box with no state runs no container', async () => {
    const { root } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    await runRm(ctx, 'demo', true)
    expect(ctx.calls.some((c) => c.args[0] === 'run')).toBe(false)
  })

  test('a docker that cannot list them does not stop the teardown', async () => {
    const { root, box } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', psArgs('demo')], new Error('daemon busy'))
    expect(await boxAppDbs(ctx, 'demo')).toEqual([])
    await runRm(ctx, 'demo', true)
    expect(ctx.calls.filter((c) => c.args.includes('down'))).toHaveLength(2)
    expect(await Bun.file(join(box, 'box.env')).exists()).toBe(false)
  })
})
