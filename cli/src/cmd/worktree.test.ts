// worktree.sh's decision table: the triple port slot, the guard rails, and
// the generated ui/.env (marker + own URLs + shared encryption root).

import { describe, expect, test } from 'bun:test'
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { WORKTREE_ENV_KEYS, runWorktree, seedFleetPlane, stripOwnKeys, worktreeSlot } from './worktree'
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
    // The chassis template is tracked in the checkout `talaria worktree` runs
    // from; the fixture carries it so the seed below has something to copy.
    mkdirSync(join(root, 'scripts'), { recursive: true })
    writeFileSync(join(root, 'scripts/chassis.template.yml'), 'services:\n  agent:\n    image: hermes\n')
    writeFileSync(
      join(root, 'ui/.env'),
      // The fixture carries TALARIA_RUST_API_URL because `talaria setup`
      // really writes it, hardcoded to :5274. Leaving it out of the fixture
      // is what hid the bug below: the worktree inherited main's url, the
      // derivation in dev.ts never fired, and the stack proxied /api/* to
      // the primary api on the primary database.
      'DATABASE_URL=postgres://t:t@127.0.0.1:5544/talaria\nREDIS_URL=redis://x\nPORT=5273\nTALARIA_RUST_API_URL=http://127.0.0.1:5274\nTALARIA_SECRET_KEY=rootkey\nAUTH_SECRET=authkey\n',
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
    // ...and the URL the app's proxy dials has to move WITH it. dev.ts derives
    // the url from the port only when no env names it, and main's ui/.env
    // names it — so inheriting that line put the worktree's app back on the
    // primary's api, on the primary's database, with the worktree's own api
    // listening to nobody. Exactly one line, and it is this stack's.
    expect(lines.filter((l) => l.startsWith('TALARIA_RUST_API_URL='))).toHaveLength(1)
    expect(lines).toContain(`TALARIA_RUST_API_URL=http://127.0.0.1:${port + 100}`)
    // The worktree's agents call back to THIS app and run in their own
    // compose project: without these the renderer aims agents at :5273 and
    // files their containers under the shared `talaria-fleet`.
    expect(lines).toContain(`TALARIA_GATEWAY_SELF_URL=http://host.docker.internal:${port}/api/llm/v1`)
    expect(lines).toContain('TALARIA_FLEET_PROJECT=talaria-wt-demo-fleet')
    // ...and the fleet plane exists for the renderer to write into — a hire
    // against a missing fleet/.env fails rather than inventing one.
    expect(existsSync(join(wt, 'fleet/agents'))).toBe(true)
    expect(readFileSync(join(wt, 'fleet/.env'), 'utf8')).toContain('LLM_BASE_URL=')
    // ...and the chassis is seeded from the template, as `talaria setup` does
    // for the primary. `fleet/` is gitignored, so the checkout never has one,
    // and without it the first `POST /api/fleet/render` 500s with "fleet
    // chassis missing" until someone copies the template by hand.
    expect(readFileSync(join(wt, 'fleet/chassis.yml'), 'utf8')).toBe('services:\n  agent:\n    image: hermes\n')
    expect(ctx.logLines).toContainEqual({ kind: 'ok', msg: 'fleet/chassis.yml (from template)' })

    // The strip-list is DERIVED from what the generated block writes, and this
    // is the guard that keeps it so: every KEY= line after the worktree marker
    // is in WORKTREE_ENV_KEYS, and every key in WORKTREE_ENV_KEYS appears in
    // the file exactly once. A key appended here but missing from the list is
    // the shape of both earlier bugs (TALARIA_API_PORT, then
    // TALARIA_RUST_API_URL): main's line survives the copy, envValue takes
    // the first match, and the worktree quietly runs on the primary's value.
    const marker = lines.findIndex((l) => l.includes('isolated worktree "demo"'))
    expect(marker).toBeGreaterThan(0)
    const written = lines.slice(marker).flatMap((l) => {
      const m = /^([A-Z_][A-Z0-9_]*)=/.exec(l)
      return m ? [m[1]!] : []
    })
    expect(new Set(written)).toEqual(new Set(WORKTREE_ENV_KEYS))
    for (const key of WORKTREE_ENV_KEYS) {
      expect(lines.filter((l) => l.startsWith(`${key}=`))).toHaveLength(1)
    }

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

  test("a devbox's gateway url, fleet project and marker are stripped, not inherited", async () => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
    // The source env is what `talaria box new` writes: a worktree cut from
    // inside a devbox copies THIS. Left in place, envValue's first match
    // would aim the worktree's agents at the box's app and file their
    // containers under the box's fleet project — the TALARIA_API_PORT and
    // TALARIA_RUST_API_URL bug a third time, on the fleet side.
    writeFileSync(
      join(root, 'ui/.env'),
      [
        'DATABASE_URL=postgres://talaria:talaria@postgres:5432/talaria',
        'TALARIA_GATEWAY_SELF_URL=http://devbox:5274/api/llm/v1',
        'TALARIA_FLEET_PROJECT=talaria-box-alpha-fleet',
        'TALARIA_WORKTREE=older',
        'TALARIA_RUST_API_URL=http://127.0.0.1:5274',
        'TALARIA_API_BIND=0.0.0.0',
        'TALARIA_SECRET_KEY=rootkey',
        '',
      ].join('\n'),
    )
    const wt = join(root, '..', 'talaria-frombox')
    rmSync(wt, { recursive: true, force: true })
    const ctx = fakeCtx()
    ctx.root = root
    try {
      await runWorktree(ctx, 'frombox')
      const env = readFileSync(join(wt, 'ui/.env'), 'utf8')
      const lines = env.split('\n')
      const port = Number(env.match(/^PORT=(\d+)$/m)![1])
      for (const [key, own] of [
        ['TALARIA_GATEWAY_SELF_URL', `http://host.docker.internal:${port}/api/llm/v1`],
        ['TALARIA_FLEET_PROJECT', 'talaria-wt-frombox-fleet'],
        ['TALARIA_WORKTREE', 'frombox'],
        ['TALARIA_RUST_API_URL', `http://127.0.0.1:${port + 100}`],
      ] as const) {
        const got = lines.filter((l) => l.startsWith(`${key}=`))
        expect(got).toEqual([`${key}=${own}`])
      }
      expect(env).not.toContain('devbox:5274')
      expect(env).not.toContain('box-alpha')
      // Lines this stack does not rewrite ride through untouched — the list
      // is what is written, nothing wider.
      expect(lines).toContain('TALARIA_API_BIND=0.0.0.0')
      expect(lines).toContain('TALARIA_SECRET_KEY=rootkey')
    } finally {
      rmSync(wt, { recursive: true, force: true })
    }
  })
})

describe('stripOwnKeys', () => {
  test('drops exactly the keys the block writes, by whole key', () => {
    const out = stripOwnKeys('PORT=1\nPORTAL=2\nTALARIA_API_PORT=3\nTALARIA_API_PORT_X=4\n# PORT=5\nOTHER=6\n')
    expect(out.split('\n')).toEqual(['PORTAL=2', 'TALARIA_API_PORT_X=4', '# PORT=5', 'OTHER=6', ''])
  })
})

describe('seedFleetPlane', () => {
  const fixture = (): { root: string; dir: string } => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-fleet-'))
    mkdirSync(join(root, 'scripts'), { recursive: true })
    writeFileSync(join(root, 'scripts/chassis.template.yml'), 'services:\n  agent: {}\n')
    const dir = join(root, 'wt')
    mkdirSync(dir)
    return { root, dir }
  }

  test('seeds fleet/chassis.yml from the template, with the dirs and fleet/.env', () => {
    const { root, dir } = fixture()
    const ctx = fakeCtx()
    seedFleetPlane(ctx, root, dir)
    expect(readFileSync(join(dir, 'fleet/chassis.yml'), 'utf8')).toBe('services:\n  agent: {}\n')
    for (const d of ['fleet/skills', 'fleet/hooks', 'fleet/agents']) expect(existsSync(join(dir, d))).toBe(true)
    expect(readFileSync(join(dir, 'fleet/.env'), 'utf8')).toContain('LLM_BASE_URL=')
    expect(ctx.logLines).toContainEqual({ kind: 'ok', msg: 'fleet/chassis.yml (from template)' })
    rmSync(root, { recursive: true, force: true })
  })

  test('an existing chassis and fleet/.env are kept — they are user-owned config', () => {
    const { root, dir } = fixture()
    mkdirSync(join(dir, 'fleet'), { recursive: true })
    writeFileSync(join(dir, 'fleet/chassis.yml'), 'services:\n  agent:\n    mine: true\n')
    writeFileSync(join(dir, 'fleet/.env'), 'LLM_BASE_URL=http://mine\n')
    const ctx = fakeCtx()
    seedFleetPlane(ctx, root, dir)
    expect(readFileSync(join(dir, 'fleet/chassis.yml'), 'utf8')).toBe('services:\n  agent:\n    mine: true\n')
    expect(readFileSync(join(dir, 'fleet/.env'), 'utf8')).toBe('LLM_BASE_URL=http://mine\n')
    expect(ctx.logLines).toContainEqual({ kind: 'skip', msg: 'fleet/chassis.yml (exists, kept)' })
    rmSync(root, { recursive: true, force: true })
  })

  test('a missing template warns instead of skipping silently', () => {
    const { root, dir } = fixture()
    rmSync(join(root, 'scripts/chassis.template.yml'))
    const ctx = fakeCtx()
    seedFleetPlane(ctx, root, dir)
    expect(existsSync(join(dir, 'fleet/chassis.yml'))).toBe(false)
    expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes('chassis.yml not seeded'))).toBe(true)
    // the rest of the plane still lands
    expect(existsSync(join(dir, 'fleet/.env'))).toBe(true)
    rmSync(root, { recursive: true, force: true })
  })
})
