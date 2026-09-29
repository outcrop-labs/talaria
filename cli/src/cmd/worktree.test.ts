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
  test('first slot with all four ports free', async () => {
    const slot = await worktreeSlot(async () => false)
    expect(slot).toEqual({ app: 5301, api: 5401, pg: 5601, redis: 6501 })
  })

  test('skips slots where any one of the four is taken', async () => {
    const taken = (p: number): Promise<boolean> =>
      Promise.resolve(p === 5301 || p === 5402 || p === 5603 || p === 6504)
    // slot 1: app taken; slot 2: api taken; slot 3: pg taken; slot 4: redis
    // taken → slot 5. The api port is in this list because it used not to be:
    // every worktree shared :5274, and `talaria dev` adopts an api already
    // there, so the second stack proxied to the first stack's database.
    const slot = await worktreeSlot(taken)
    expect(slot).toEqual({ app: 5305, api: 5405, pg: 5605, redis: 6505 })
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

    // MAIN's database is proved ready BEFORE the dump reads it. `docker inspect`
    // only answers for the container, and a postgres still running its
    // entrypoint has no `talaria` database yet — so the seed used to die with
    // `FATAL: database "talaria" does not exist` after the worktree and both
    // containers existed and before ui/.env was written, which is the step that
    // makes the checkout usable at all.
    const mainReady = ctx.calls.findIndex(
      (c) => c.args.includes('pg_isready') && c.args.includes('talaria-postgres-dev'),
    )
    const dump = ctx.calls.findIndex((c) => c.args.includes('pg_dump'))
    expect(mainReady).toBeGreaterThanOrEqual(0)
    expect(mainReady).toBeLessThan(dump)

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
    // the app/api/pg/redis ports share one slot number
    const port = Number(env.match(/^PORT=(\d+)$/m)![1])
    expect(Number(ctx.env.TALARIA_PG_PORT)).toBe(port + 300)
    expect(Number(ctx.env.TALARIA_REDIS_PORT)).toBe(port + 1200)
    // The api gets its OWN port. Without this the Rust api stayed on the fixed
    // :5274 for every stack, and `talaria dev` adopts an api already listening
    // there — so a second worktree proxied /api/* to the first worktree's api,
    // on the first worktree's database.
    expect(lines.filter((l) => l.startsWith('TALARIA_API_PORT='))).toHaveLength(1)
    expect(lines).toContain(`TALARIA_API_PORT=${port + 100}`)

      // node_modules shared by symlink
      expect(lstatSync(join(wt, 'ui/node_modules')).isSymbolicLink()).toBe(true)
      expect(readlinkSync(join(wt, 'ui/node_modules'))).toBe(join(root, 'ui/node_modules'))
    } finally {
      rmSync(wt, { recursive: true, force: true })
    }
  })

  test("main's own TALARIA_API_PORT is stripped, not inherited", async () => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
    // Main names the port explicitly — a perfectly normal thing for it to do.
    writeFileSync(
      join(root, 'ui/.env'),
      'DATABASE_URL=postgres://t:t@127.0.0.1:5544/talaria\nTALARIA_API_PORT=5274\nTALARIA_SECRET_KEY=rootkey\n',
    )
    const wt = join(root, '..', 'talaria-inherit')
    rmSync(wt, { recursive: true, force: true })
    const ctx = fakeCtx()
    ctx.root = root
    try {
      await runWorktree(ctx, 'inherit')
      const lines = readFileSync(join(wt, 'ui/.env'), 'utf8').split('\n')
      // ONE line, and it is the slot's — not main's. envValue returns the FIRST
      // match, so leaving main's line above the appended one would have shadowed
      // it and quietly put this worktree's api back on the shared :5274, which
      // is the whole bug the per-stack port exists to prevent.
      const api = lines.filter((l) => l.startsWith('TALARIA_API_PORT='))
      expect(api).toHaveLength(1)
      expect(api[0]).not.toBe('TALARIA_API_PORT=5274')
      expect(api[0]).toMatch(/^TALARIA_API_PORT=54\d\d$/)
    } finally {
      rmSync(wt, { recursive: true, force: true })
    }
  })
})
