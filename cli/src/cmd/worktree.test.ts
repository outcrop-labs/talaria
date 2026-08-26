// worktree.sh's decision table: the triple port slot, the guard rails, and
// the generated ui/.env (marker + own URLs + shared encryption root).

import { describe, expect, test } from 'bun:test'
import { lstatSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { runWorktree, worktreeSlot } from './worktree'
import { fakeCtx, type FakeCtx } from '../testing'
import { CliError } from '../ui'

const attempt = async (fn: () => Promise<unknown>): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

describe('worktreeSlot', () => {
  test('first slot with all three ports free', async () => {
    const slot = await worktreeSlot(async () => false)
    expect(slot).toEqual({ app: 5301, pg: 5601, redis: 6501 })
  })

  test('skips slots where any one of the three is taken', async () => {
    const taken = (p: number): Promise<boolean> => Promise.resolve(p === 5301 || p === 5602 || p === 6503)
    // slot 1: app taken; slot 2: pg taken; slot 3: redis taken → slot 4
    const slot = await worktreeSlot(taken)
    expect(slot).toEqual({ app: 5304, pg: 5604, redis: 6504 })
  })

  test('null when the range is exhausted', async () => {
    expect(await worktreeSlot(async () => true)).toBeNull()
  })
})

describe('runWorktree — guards', () => {
  test('bad name dies before anything', async () => {
    const ctx = fakeCtx()
    ctx.root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    const msg = await attempt(() => runWorktree(ctx, 'Bad_Name'))
    expect(msg).toContain('lowercase-kebab')
    expect(ctx.calls.length).toBe(0)
  })

  test('missing main ui/.env dies pointing at setup', async () => {
    const ctx = fakeCtx()
    ctx.root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    const msg = await attempt(() => runWorktree(ctx, 'demo'))
    expect(msg).toContain('bun talaria setup')
  })

  test('main postgres down dies before creating anything', async () => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    mkdirSync(join(root, 'ui'), { recursive: true })
    writeFileSync(join(root, 'ui/.env'), 'DATABASE_URL=x\n')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', ['inspect', 'talaria-postgres-dev']], new Error('no such object'))
    const msg = await attempt(() => runWorktree(ctx, 'demo'))
    expect(msg).toContain("isn't running")
    expect(ctx.calls.some((c) => c.args[0] === 'worktree')).toBe(false)
  })
})

describe('runWorktree — happy path', () => {
  test('creates the stack, seeds, stamps ui/.env, links node_modules', async () => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
    writeFileSync(
      join(root, 'ui/.env'),
      'DATABASE_URL=postgres://t:t@127.0.0.1:5544/talaria\nREDIS_URL=redis://x\nPORT=5273\nTALARIA_SECRET_KEY=rootkey\nAUTH_SECRET=authkey\n',
    )
    // `git worktree add` is planted (fake success, creates nothing): the
    // module's own mkdir carries the ui/.env write from here.
    const wt = join(root, '..', 'talaria-demo')
    rmSync(wt, { recursive: true, force: true }) // a previous run may have leaked it into /tmp
    const ctx = fakeCtx()
    ctx.root = root
    try {
      await runWorktree(ctx, 'demo')

    // the worktree compose project, with its own containers/ports interpolated,
    // scoped to the two services the worktree owns (the sidecars carry fixed
    // names/ports shared with main — see the comment in runWorktree)
    const up = ctx.calls.find((c) => c.args.includes('up') && c.args.includes('-d'))!
    expect(up.args).toContain('-p')
    expect(up.args).toContain('talaria-wt-demo')
    expect(up.args.slice(-2)).toEqual(['postgres', 'redis'])
    expect(ctx.env.TALARIA_PG_CONTAINER).toBe('talaria-pg-demo')
    expect(ctx.env.TALARIA_PG_PORT).toMatch(/^56\d\d$/)
    expect(ctx.env.TALARIA_REDIS_PORT).toMatch(/^65\d\d$/)

    // the seed pipe: main's dump into the worktree's postgres
    expect(ctx.calls.some((c) => c.args.includes('pg_dump'))).toBe(true)
    expect(ctx.calls.some((c) => c.args.includes('-i') && c.args.includes('talaria-pg-demo'))).toBe(true)

    const env = readFileSync(join(wt, 'ui/.env'), 'utf8')
    const lines = env.split('\n')
    // strip-list: only the three service lines; the secret keys ride verbatim
    expect(lines).toContain('TALARIA_SECRET_KEY=rootkey')
    expect(lines).toContain('AUTH_SECRET=authkey')
    expect(lines.filter((l) => l.startsWith('DATABASE_URL='))).toHaveLength(1)
    expect(lines).toContain(`DATABASE_URL=postgres://talaria:talaria@127.0.0.1:${ctx.env.TALARIA_PG_PORT}/talaria`)
    expect(lines).toContain(`REDIS_URL=redis://127.0.0.1:${ctx.env.TALARIA_REDIS_PORT}`)
    expect(lines).toContain('TALARIA_WORKTREE=demo')
    expect(lines.some((l) => l.startsWith('PORT=53'))).toBe(true)
    // the app/pg/redis ports share one slot number
    const port = Number(env.match(/^PORT=(\d+)$/m)![1])
    expect(Number(ctx.env.TALARIA_PG_PORT)).toBe(port + 300)
    expect(Number(ctx.env.TALARIA_REDIS_PORT)).toBe(port + 1200)

      // node_modules shared by symlink
      expect(lstatSync(join(wt, 'ui/node_modules')).isSymbolicLink()).toBe(true)
      expect(readlinkSync(join(wt, 'ui/node_modules'))).toBe(join(root, 'ui/node_modules'))
    } finally {
      rmSync(wt, { recursive: true, force: true })
    }
  })
})
