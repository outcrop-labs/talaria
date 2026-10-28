// worktree.sh's decision table: the triple port slot, the guard rails, and
// the generated ui/.env (marker + own URLs + shared encryption root).

import { describe, expect, test } from 'bun:test'
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  CLEAR_SEEDED_GATEWAY_PORTS,
  COUNT_TABLES,
  WORKTREE_ENV_KEYS,
  isSideStack,
  runWorktree,
  seedFleetPlane,
  setupDiffers,
  sharedUiNodeModules,
  slotPorts,
  stripOwnKeys,
  worktreeSlot,
} from './worktree'
import { WORKTREE_COMPOSE, pgReadyArgs } from '../compose'
import { fakeCtx, type FakeCtx } from '../testing'
import { CliError } from '../ui'

/** The read that says which compose project a container belongs to. */
const INSPECT_PROJECT = (c: string) => ['inspect', '-f', '{{index .Config.Labels "com.docker.compose.project"}}', c]
/** The table count runWorktree reads on each end of the seed. */
const COUNT = (c: string) => ['exec', c, 'psql', '-U', 'talaria', '-d', 'talaria', '-tAc', COUNT_TABLES]

const attempt = async (fn: () => Promise<unknown>): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

describe('worktreeSlot', () => {
  test('first slot with every port free', async () => {
    const slot = await worktreeSlot(async () => false)
    expect(slot).toEqual({
      app: 5301,
      api: 5401,
      pg: 5601,
      redis: 6501,
      mcp: 5701,
      ompAuth: 5801,
      gatewayBase: 20100,
    })
  })

  test('skips slots where any one of the ports is taken', async () => {
    const taken = (p: number): Promise<boolean> =>
      Promise.resolve(
        [5301, 5402, 5603, 6504, 5705, 5806, 20700].includes(p),
      )
    // slots 1-4: app, api, pg, redis taken; 5: toolkit; 6: omp auth;
    // 7: the gateway range's first port → slot 8. The api port is in this
    // list because it used not to be: every worktree shared :5274, and
    // `talaria dev` adopts an api already there, so the second stack proxied
    // to the first stack's database. The toolkit, omp-auth and gateway ports
    // are here for the same reason, one hop further out (see worktreeSlot).
    const slot = await worktreeSlot(taken)
    expect(slot).toEqual(slotPorts(8))
  })

  test("a slot's ranges never overlap another slot's, or main's fixed ports", () => {
    const main = [5273, 5274, 5276, 5280, 5544, 8770]
    const seen = new Set<number>()
    for (let c = 1; c <= 89; c++) {
      const s = slotPorts(c)
      const gateway = Array.from({ length: 100 }, (_, i) => s.gatewayBase + i)
      for (const port of [s.app, s.api, s.pg, s.redis, s.mcp, s.ompAuth, ...gateway]) {
        expect(main).not.toContain(port)
        expect(seen.has(port)).toBe(false)
        seen.add(port)
      }
    }
    // main's agents allocate upward from 8770; the worktree ranges start far above.
    expect(Math.min(...[...seen].filter((p) => p >= 8770))).toBeGreaterThan(8770 + 1000)
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
    ctx.plant(['docker', INSPECT_PROJECT('talaria-postgres-dev')], new Error('no such object'))
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
    // ...with the worktree file LAST: both on the default bridge, so the
    // project creates no network and takes no subnet from docker's pools.
    const files = up.args.flatMap((a, i) => (up.args[i - 1] === '-f' ? [a] : []))
    expect(files.at(-1)).toBe(join(root, WORKTREE_COMPOSE))
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
    //
    // Both waits are a real query over TCP (pgReadyArgs), and the worktree's
    // comes after its `up` and before the restore: the socket pg_isready they
    // were passed against the entrypoint's temporary init server, and the
    // seed then died with "the database system is shutting down".
    const at = (args: string[]) => ctx.calls.findIndex((c) => JSON.stringify(c.args) === JSON.stringify(args))
    const mainReady = at(pgReadyArgs('talaria-postgres-dev'))
    const ownReady = at(pgReadyArgs('talaria-pg-demo'))
    const upAt = ctx.calls.indexOf(up)
    const dump = ctx.calls.findIndex((c) => c.args.includes('pg_dump'))
    expect(mainReady).toBeGreaterThanOrEqual(0)
    expect(mainReady).toBeLessThan(dump)
    expect(ownReady).toBeGreaterThan(upAt)
    expect(ownReady).toBeLessThan(dump)
    // ...and main is asked once more right before the dump, after this
    // stack's own wait: a main restarted mid-creation failed the dump.
    const mainProbe = JSON.stringify(pgReadyArgs('talaria-postgres-dev'))
    const mainAgain = ctx.calls.map((c) => JSON.stringify(c.args)).lastIndexOf(mainProbe)
    expect(mainAgain).toBeGreaterThan(ownReady)
    expect(mainAgain).toBeLessThan(dump)
    expect(ctx.calls.some((c) => c.args.includes('pg_isready'))).toBe(false)

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
    // ...and the api's loopback children and the agents' gateway ports move
    // with the slot: on the shared :5280 the second stack's agents were served
    // by the first stack's toolkit (401), and every stack's first agent asked
    // for :8770.
    const slotNo = port - 5300
    expect(lines).toContain(`TALARIA_MCP_PORT=${5700 + slotNo}`)
    expect(lines).toContain(`TALARIA_OMP_AUTH_PORT=${5800 + slotNo}`)
    expect(lines).toContain(`TALARIA_GATEWAY_PORT_BASE=${20000 + slotNo * 100}`)
    // ...and the gateway ports the seed copied from main are cleared in THIS
    // stack's database, after the restore, so its first render assigns its own.
    const restore = ctx.calls.findIndex((c) => c.args.includes('-i') && c.args.includes('talaria-pg-demo'))
    const clear = ctx.calls.findIndex(
      (c) => c.args.includes('talaria-pg-demo') && c.args.includes(CLEAR_SEEDED_GATEWAY_PORTS),
    )
    expect(clear).toBeGreaterThan(restore)
    expect(ctx.calls.some((c) => c.args.includes(CLEAR_SEEDED_GATEWAY_PORTS) && c.args.includes('talaria-postgres-dev'))).toBe(false)
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
    // A checkout with no worktree.ts of its own has nothing to compare.
    expect(ctx.logLines.filter((l) => l.kind === 'warn')).toEqual([])

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

      // node_modules shared by symlink — git named no primary here (the
      // fake answers nothing), so the invoking checkout's, resolved
      expect(lstatSync(join(wt, 'ui/node_modules')).isSymbolicLink()).toBe(true)
      expect(readlinkSync(join(wt, 'ui/node_modules'))).toBe(realpathSync(join(root, 'ui/node_modules')))
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

describe('runWorktree — readiness, and what a timed-out wait stops', () => {
  const fixture = (name: string) => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
    writeFileSync(join(root, 'ui/.env'), 'DATABASE_URL=postgres://t:t@127.0.0.1:5544/talaria\nTALARIA_SECRET_KEY=rootkey\n')
    const wt = join(root, '..', `talaria-${name}`)
    rmSync(wt, { recursive: true, force: true })
    const ctx = fakeCtx()
    ctx.root = root
    return { root, wt, ctx }
  }

  test("a worktree postgres that never serves its database stops the command before the seed", async () => {
    const { root, wt, ctx } = fixture('neverready')
    ctx.plant(['docker', pgReadyArgs('talaria-pg-neverready')], new Error('the database system is shutting down'))
    try {
      const msg = await attempt(() => runWorktree(ctx, 'neverready'))
      expect(msg).toContain("never served its 'talaria' database")
      // ...and says how to clear the half-made stack it leaves.
      expect(msg).toContain('docker compose -p talaria-wt-neverready down -v')
      // The wait's answer is READ now: it used to be discarded, and the seed
      // ran straight into the temp server's shutdown.
      expect(ctx.calls.some((c) => c.args.includes('pg_dump'))).toBe(false)
      expect(existsSync(join(wt, 'ui/.env'))).toBe(false)
    } finally {
      rmSync(wt, { recursive: true, force: true })
      rmSync(root, { recursive: true, force: true })
    }
  })

  test("main's database never answering stops it before anything is created", async () => {
    const { root, wt, ctx } = fixture('mainasleep')
    ctx.plant(['docker', pgReadyArgs('talaria-postgres-dev')], new Error('database "talaria" does not exist'))
    try {
      const msg = await attempt(() => runWorktree(ctx, 'mainasleep'))
      expect(msg).toContain("'talaria' database never became ready")
      expect(ctx.calls.some((c) => c.args[0] === 'worktree')).toBe(false)
    } finally {
      rmSync(wt, { recursive: true, force: true })
      rmSync(root, { recursive: true, force: true })
    }
  })

  test('cli/ and mcp/ are installed from their own lockfiles; ui/ is linked', async () => {
    const { root, wt, ctx } = fixture('deps')
    const exec = ctx.exec
    ctx.exec = async (cmd, args, opts) => {
      // What a real checkout of the base ref carries: the two packages, and
      // no node_modules in either (they are gitignored).
      if (cmd === 'git' && args[0] === 'worktree' && args[1] === 'add') {
        for (const pkg of ['cli', 'mcp']) {
          mkdirSync(join(wt, pkg), { recursive: true })
          writeFileSync(join(wt, pkg, 'package.json'), '{}\n')
        }
      }
      return exec(cmd, args, opts)
    }
    try {
      await runWorktree(ctx, 'deps')
      const installs = ctx.calls.filter((c) => c.cmd === 'bun' && c.args[0] === 'install')
      // The gate typechecks both when the diff touches them, and a fresh
      // worktree had neither, so its first gate failed on a missing tsc.
      expect(installs.map((c) => [c.args, c.opts?.cwd])).toEqual([
        [['install', '--frozen-lockfile'], join(wt, 'cli')],
        [['install', '--frozen-lockfile'], join(wt, 'mcp')],
      ])
      expect(lstatSync(join(wt, 'ui/node_modules')).isSymbolicLink()).toBe(true)
    } finally {
      rmSync(wt, { recursive: true, force: true })
      rmSync(root, { recursive: true, force: true })
    }
  })

  test('a failed install warns and the stack is still made — the gate installs it later', async () => {
    const { root, wt, ctx } = fixture('nodeps')
    const exec = ctx.exec
    ctx.exec = async (cmd, args, opts) => {
      if (cmd === 'git' && args[0] === 'worktree' && args[1] === 'add') {
        mkdirSync(join(wt, 'cli'), { recursive: true })
        writeFileSync(join(wt, 'cli/package.json'), '{}\n')
      }
      return exec(cmd, args, opts)
    }
    ctx.plant(['bun', ['install', '--frozen-lockfile']], new Error('offline'), join(wt, 'cli'))
    try {
      expect(await runWorktree(ctx, 'nodeps')).toBe(0)
      expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes('bun install in cli/ failed'))).toBe(true)
      expect(existsSync(join(wt, 'ui/.env'))).toBe(true)
    } finally {
      rmSync(wt, { recursive: true, force: true })
      rmSync(root, { recursive: true, force: true })
    }
  })
})

describe('runWorktree — the base ref\'s own setup', () => {
  // `talaria worktree` runs the CLI of the checkout it is invoked from — the
  // primary, usually on main — so a stack cut from rc took main's chassis
  // and main's env rules, and said nothing. `git worktree add` is planted
  // here to leave the files a real checkout of the base ref would carry.
  test("the worktree's own chassis template wins, and a differing setup warns", async () => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
    mkdirSync(join(root, 'scripts'), { recursive: true })
    writeFileSync(join(root, 'scripts/chassis.template.yml'), 'image: hermes:older\n')
    writeFileSync(join(root, 'ui/.env'), 'DATABASE_URL=postgres://t:t@127.0.0.1:5544/talaria\nTALARIA_SECRET_KEY=rootkey\n')
    const wt = join(root, '..', 'talaria-fromrc')
    rmSync(wt, { recursive: true, force: true })
    const ctx = fakeCtx()
    ctx.root = root
    const exec = ctx.exec
    ctx.exec = async (cmd, args, opts) => {
      if (cmd === 'git' && args[0] === 'worktree' && args[1] === 'add') {
        mkdirSync(join(wt, 'scripts'), { recursive: true })
        writeFileSync(join(wt, 'scripts/chassis.template.yml'), 'image: hermes:pinned\n')
        mkdirSync(join(wt, 'cli/src/cmd'), { recursive: true })
        writeFileSync(join(wt, 'cli/src/cmd/worktree.ts'), '// the base ref writes more lines than this CLI does\n')
      }
      return exec(cmd, args, opts)
    }
    try {
      await runWorktree(ctx, 'fromrc', 'origin/rc')
      expect(readFileSync(join(wt, 'fleet/chassis.yml'), 'utf8')).toBe('image: hermes:pinned\n')
      const warns = ctx.logLines.filter((l) => l.kind === 'warn').map((l) => l.msg)
      expect(warns).toHaveLength(1)
      expect(warns[0]).toContain("differs from origin/rc's own cli/src/cmd/worktree.ts")
    } finally {
      rmSync(wt, { recursive: true, force: true })
      rmSync(root, { recursive: true, force: true })
    }
  })
})

describe('setupDiffers', () => {
  const running = fileURLToPath(new URL('./worktree.ts', import.meta.url))
  const checkout = (body?: string): string => {
    const wt = mkdtempSync(join(tmpdir(), 'talaria-wt-own-'))
    if (body !== undefined) {
      mkdirSync(join(wt, 'cli/src/cmd'), { recursive: true })
      writeFileSync(join(wt, 'cli/src/cmd/worktree.ts'), body)
    }
    return wt
  }

  test('the same setup is not a difference', () => {
    const wt = checkout(readFileSync(running, 'utf8'))
    expect(setupDiffers(running, wt)).toBe(false)
    rmSync(wt, { recursive: true, force: true })
  })

  test('a different setup is', () => {
    const wt = checkout('// another version\n')
    expect(setupDiffers(running, wt)).toBe(true)
    rmSync(wt, { recursive: true, force: true })
  })

  test('a base ref without the file has nothing to compare', () => {
    const wt = checkout()
    expect(setupDiffers(running, wt)).toBe(false)
    rmSync(wt, { recursive: true, force: true })
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

// ── ui/node_modules: the primary's, resolved (follow-up R21) ───────────────
// A worktree cut by a CLI running in ANOTHER worktree linked that worktree's
// `ui/node_modules` — itself a link to the primary's — and dangled the moment
// that worktree was removed.
describe('sharedUiNodeModules — which node_modules a new worktree links', () => {
  const tree = () => {
    const base = realpathSync(mkdtempSync(join(tmpdir(), 'talaria-nm-')))
    const primary = join(base, 'talaria')
    mkdirSync(join(primary, '.git'), { recursive: true })
    mkdirSync(join(primary, 'ui/node_modules'), { recursive: true })
    // The invoking checkout is a linked worktree: its node_modules is a link
    // to the primary's, as `talaria worktree` made it.
    const invoking = join(base, 'talaria-older')
    mkdirSync(join(invoking, 'ui'), { recursive: true })
    symlinkSync(join(primary, 'ui/node_modules'), join(invoking, 'ui/node_modules'))
    return { base, primary, invoking }
  }

  test("from a linked worktree: the primary's, found from git's common dir", async () => {
    const { base, primary, invoking } = tree()
    const ctx = fakeCtx()
    ctx.plant(['git', ['rev-parse', '--git-common-dir']], join(primary, '.git'), invoking)
    try {
      expect(await sharedUiNodeModules(ctx, invoking)).toBe(join(primary, 'ui/node_modules'))
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })

  test('from the primary itself: git answers a relative `.git`, and that resolves against it', async () => {
    const { base, primary } = tree()
    const ctx = fakeCtx()
    ctx.plant(['git', ['rev-parse', '--git-common-dir']], '.git\n', primary)
    try {
      expect(await sharedUiNodeModules(ctx, primary)).toBe(join(primary, 'ui/node_modules'))
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })

  test("git cannot say: the invoking checkout's own, resolved through every link", async () => {
    const { base, primary, invoking } = tree()
    const ctx = fakeCtx()
    ctx.plant(['git', ['rev-parse', '--git-common-dir']], new Error('fatal: not a git repository'), invoking)
    try {
      // Not `<invoking>/ui/node_modules`: that is the link that dangles.
      expect(await sharedUiNodeModules(ctx, invoking)).toBe(join(primary, 'ui/node_modules'))
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })

  test("a primary with no install: the invoking checkout's own, resolved", async () => {
    const base = realpathSync(mkdtempSync(join(tmpdir(), 'talaria-nm-')))
    const primary = join(base, 'talaria')
    mkdirSync(join(primary, '.git'), { recursive: true })
    const elsewhere = join(base, 'cache/node_modules')
    mkdirSync(elsewhere, { recursive: true })
    const invoking = join(base, 'talaria-older')
    mkdirSync(join(invoking, 'ui'), { recursive: true })
    symlinkSync(elsewhere, join(invoking, 'ui/node_modules'))
    const ctx = fakeCtx()
    ctx.plant(['git', ['rev-parse', '--git-common-dir']], join(primary, '.git'), invoking)
    try {
      expect(await sharedUiNodeModules(ctx, invoking)).toBe(elsewhere)
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })

  test('a bare common dir names no primary checkout', async () => {
    const { base, invoking, primary } = tree()
    const ctx = fakeCtx()
    ctx.plant(['git', ['rev-parse', '--git-common-dir']], join(base, 'talaria.git'), invoking)
    try {
      // falls through to the invoking checkout's, resolved — still the real dir
      expect(await sharedUiNodeModules(ctx, invoking)).toBe(join(primary, 'ui/node_modules'))
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })

  test('nothing anywhere: null', async () => {
    const base = realpathSync(mkdtempSync(join(tmpdir(), 'talaria-nm-')))
    mkdirSync(join(base, 'talaria/ui'), { recursive: true })
    const ctx = fakeCtx()
    try {
      expect(await sharedUiNodeModules(ctx, join(base, 'talaria'))).toBeNull()
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })

  test('runWorktree from a worktree links the primary, and the link outlives the invoking worktree', async () => {
    const { base, primary, invoking } = tree()
    writeFileSync(join(invoking, 'ui/.env'), 'DATABASE_URL=postgres://t:t@127.0.0.1:5544/talaria\nTALARIA_SECRET_KEY=rootkey\n')
    const ctx = fakeCtx()
    ctx.root = invoking
    ctx.plant(['git', ['rev-parse', '--git-common-dir']], join(primary, '.git'), invoking)
    const wt = join(base, 'talaria-fromwt')
    try {
      await runWorktree(ctx, 'fromwt')
      expect(readlinkSync(join(wt, 'ui/node_modules'))).toBe(join(primary, 'ui/node_modules'))
      // The cleanup that removes the invoking worktree no longer takes this
      // one's toolchain with it.
      rmSync(invoking, { recursive: true, force: true })
      expect(existsSync(join(wt, 'ui/node_modules'))).toBe(true)
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })

  test('runWorktree with nothing to link warns and leaves no dangling link', async () => {
    const base = realpathSync(mkdtempSync(join(tmpdir(), 'talaria-nm-')))
    const root = join(base, 'talaria')
    mkdirSync(join(root, 'ui'), { recursive: true })
    writeFileSync(join(root, 'ui/.env'), 'DATABASE_URL=postgres://t:t@127.0.0.1:5544/talaria\nTALARIA_SECRET_KEY=rootkey\n')
    const ctx = fakeCtx()
    ctx.root = root
    const wt = join(base, 'talaria-nolink')
    try {
      await runWorktree(ctx, 'nolink')
      expect(() => lstatSync(join(wt, 'ui/node_modules'))).toThrow()
      expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes('no ui/node_modules'))).toBe(true)
    } finally {
      rmSync(base, { recursive: true, force: true })
    }
  })
})

// ── The seed says what it copied, and from where (follow-up R22) ───────────
// Main's dev database was empty on this host, so every worktree "seeded (a
// point-in-time copy of main)" and copied nothing. And a restore that broke
// half way exited 0, because psql does without ON_ERROR_STOP.
describe('runWorktree — what the seed copied', () => {
  const fixture = (name: string, env: Record<string, string> = {}) => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-wt-'))
    mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
    writeFileSync(join(root, 'ui/.env'), 'DATABASE_URL=postgres://t:t@127.0.0.1:5544/talaria\nTALARIA_SECRET_KEY=rootkey\n')
    const wt = join(root, '..', `talaria-${name}`)
    rmSync(wt, { recursive: true, force: true })
    const ctx = fakeCtx({ env })
    ctx.root = root
    const done = () => {
      rmSync(wt, { recursive: true, force: true })
      rmSync(root, { recursive: true, force: true })
    }
    return { root, wt, ctx, done }
  }
  const restoreOf = (ctx: FakeCtx, pgc: string) => ctx.calls.find((c) => c.args.includes('-i') && c.args.includes(pgc))

  test("an empty main says so — it never claims a copy — and the dump still ran", async () => {
    const { ctx, done } = fixture('emptymain')
    ctx.plant(['docker', COUNT('talaria-postgres-dev')], '0\n')
    try {
      await runWorktree(ctx, 'emptymain')
      const warn = ctx.logLines.find((l) => l.kind === 'warn' && l.msg.includes('has no tables'))
      expect(warn?.msg).toContain("main's database (talaria-postgres-dev) has no tables")
      expect(warn?.msg).toContain('this worktree starts empty, and the app migrates it on its first `talaria dev`')
      expect(ctx.logLines.some((l) => l.kind === 'ok' && l.msg.startsWith('seeded'))).toBe(false)
      // A dump or restore that fails still has to stop the command, so the
      // pipe runs even when there is nothing to copy.
      expect(ctx.calls.some((c) => c.args.includes('pg_dump'))).toBe(true)
    } finally {
      done()
    }
  })

  test('a full copy says how many tables it carried', async () => {
    const { ctx, done } = fixture('fullcopy')
    ctx.plant(['docker', COUNT('talaria-postgres-dev')], '142\n')
    ctx.plant(['docker', COUNT('talaria-pg-fullcopy')], '142\n')
    try {
      await runWorktree(ctx, 'fullcopy')
      expect(ctx.logLines).toContainEqual({ kind: 'ok', msg: 'seeded (a point-in-time copy of main: 142 tables)' })
      // main is counted BEFORE the dump reads it, this stack after the restore
      const at = (args: string[]) => ctx.calls.findIndex((c) => JSON.stringify(c.args) === JSON.stringify(args))
      const dump = ctx.calls.findIndex((c) => c.args.includes('pg_dump'))
      expect(at(COUNT('talaria-postgres-dev'))).toBeLessThan(dump)
      expect(at(COUNT('talaria-pg-fullcopy'))).toBeGreaterThan(dump)
    } finally {
      done()
    }
  })

  test("a copy whose own count cannot be read says so — it never quotes main's as checked", async () => {
    const { ctx, done } = fixture('unreadcopy')
    ctx.plant(['docker', COUNT('talaria-postgres-dev')], '142\n')
    ctx.plant(['docker', COUNT('talaria-pg-unreadcopy')], new Error('docker exited 1'))
    try {
      await runWorktree(ctx, 'unreadcopy')
      expect(ctx.logLines).toContainEqual({
        kind: 'ok',
        msg: "seeded (a point-in-time copy of main: 142 tables; this worktree's count could not be read)",
      })
      expect(ctx.logLines).not.toContainEqual({ kind: 'ok', msg: 'seeded (a point-in-time copy of main: 142 tables)' })
    } finally {
      done()
    }
  })

  test('the restore stops on the first failed statement', async () => {
    const { ctx, done } = fixture('strict')
    try {
      await runWorktree(ctx, 'strict')
      const restore = restoreOf(ctx, 'talaria-pg-strict')!
      expect(restore.args.slice(-2)).toEqual(['-v', 'ON_ERROR_STOP=1'])
    } finally {
      done()
    }
  })

  test('a restore that came up short dies with the teardown, before ui/.env', async () => {
    const { wt, ctx, done } = fixture('short')
    ctx.plant(['docker', COUNT('talaria-postgres-dev')], '142\n')
    ctx.plant(['docker', COUNT('talaria-pg-short')], '17\n')
    try {
      const msg = await attempt(() => runWorktree(ctx, 'short'))
      expect(msg).toContain('seed incomplete: main has 142 tables, this worktree\'s database 17')
      expect(msg).toContain('docker compose -p talaria-wt-short down -v')
      expect(existsSync(join(wt, 'ui/.env'))).toBe(false)
    } finally {
      done()
    }
  })

  test('a failed dump or restore dies loudly, with the teardown', async () => {
    for (const side of ['dump', 'restore'] as const) {
      const { wt, ctx, done } = fixture(`fail${side}`)
      const dumpArgs = ['exec', 'talaria-postgres-dev', 'pg_dump', '-U', 'talaria', '-d', 'talaria', '--clean', '--if-exists']
      const restoreArgs = ['exec', '-i', `talaria-pg-fail${side}`, 'psql', '-U', 'talaria', '-d', 'talaria', '-q', '-v', 'ON_ERROR_STOP=1']
      ctx.plant(['docker', side === 'dump' ? dumpArgs : restoreArgs], new Error('docker exited 3'))
      try {
        const msg = await attempt(() => runWorktree(ctx, `fail${side}`))
        expect(msg).toContain('seed dump/restore failed: docker exited 3')
        expect(msg).toContain(`docker compose -p talaria-wt-fail${side} down -v`)
        expect(existsSync(join(wt, 'ui/.env'))).toBe(false)
        expect(ctx.logLines.some((l) => l.msg.startsWith('seeded'))).toBe(false)
      } finally {
        done()
      }
    }
  })

  test("TALARIA_PG_CONTAINER naming a worktree's or a box's Postgres is refused before anything exists", async () => {
    for (const [pgc, project] of [
      ['talaria-pg-other', 'talaria-wt-other'],
      ['devbox-alpha-postgres', 'devbox-alpha'],
    ] as const) {
      const { ctx, done } = fixture('inherited', { TALARIA_PG_CONTAINER: pgc })
      ctx.plant(['docker', INSPECT_PROJECT(pgc)], `${project}\n`)
      try {
        const msg = await attempt(() => runWorktree(ctx, 'inherited'))
        expect(msg).toContain(`TALARIA_PG_CONTAINER=${pgc} is the Postgres of compose project ${project}, not main's`)
        expect(msg).toContain('env -u TALARIA_PG_CONTAINER bun talaria worktree inherited')
        expect(ctx.calls.some((c) => c.args[0] === 'worktree')).toBe(false)
        expect(ctx.calls.some((c) => c.args.includes('pg_dump'))).toBe(false)
      } finally {
        done()
      }
    }
  })

  test("an operator's own main container (no stack's project) is still honoured", async () => {
    for (const label of ['talaria-dev\n', '<no value>\n', '\n']) {
      const { ctx, done } = fixture('custommain', { TALARIA_PG_CONTAINER: 'my-postgres' })
      ctx.plant(['docker', INSPECT_PROJECT('my-postgres')], label)
      try {
        await runWorktree(ctx, 'custommain')
        expect(ctx.calls.some((c) => c.args.includes('pg_dump') && c.args.includes('my-postgres'))).toBe(true)
      } finally {
        done()
      }
    }
  })

  test('side stacks are worktrees, their fleets and boxes — never main', () => {
    expect(isSideStack('talaria-wt-x')).toBe(true)
    expect(isSideStack('talaria-wt-x-fleet')).toBe(true)
    expect(isSideStack('devbox-alpha')).toBe(true)
    expect(isSideStack('talaria-dev')).toBe(false)
    expect(isSideStack('talaria')).toBe(false)
    expect(isSideStack('')).toBe(false)
  })
})
