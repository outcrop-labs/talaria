// dev.sh's decision table, as tests: the mcp staleness probe, the skip/devbox
// gates, the worktree guard, and the S3 lift. Filesystem state is real (tmp
// trees); process behavior is planted. `die` throws CliError (the dispatcher
// is what prints it), so refusals are asserted on the throw.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'
import { apiEnv, runDev, rustApi } from './dev'
import { fakeCtx, type FakeCtx } from '../testing'
import { CliError } from '../ui'

const makeTree = (over: { distMtime?: Date; uiEnv?: string; bridgeDistMtime?: Date } = {}) => {
  const root = mkdtempSync(join(tmpdir(), 'talaria-dev-'))
  mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
  mkdirSync(join(root, 'mcp/node_modules'), { recursive: true })
  mkdirSync(join(root, 'mcp/src/deep'), { recursive: true })
  mkdirSync(join(root, 'mcp/dist'), { recursive: true })
  writeFileSync(join(root, 'mcp/src/index.ts'), 'x')
  writeFileSync(join(root, 'mcp/src/deep/util.ts'), 'x')
  writeFileSync(join(root, 'mcp/dist/index.js'), 'built')
  // The omp auth bridge builds on the same staleness rule. A fresh bundle
  // here keeps the mcp cases below about mcp: `ompAuthStale` is its own
  // describe block.
  mkdirSync(join(root, 'omp-auth/src'), { recursive: true })
  mkdirSync(join(root, 'omp-auth/dist'), { recursive: true })
  mkdirSync(join(root, 'omp-auth/node_modules'), { recursive: true })
  writeFileSync(join(root, 'omp-auth/src/server.ts'), 'x')
  writeFileSync(join(root, 'omp-auth/dist/server.js'), 'built')
  writeFileSync(join(root, 'ui/.env'), over.uiEnv ?? 'DATABASE_URL=x\nREDIS_URL=y\n')
  const distTime = over.distMtime ?? new Date()
  utimesSync(join(root, 'mcp/dist/index.js'), distTime, distTime)
  utimesSync(join(root, 'mcp/src/index.ts'), distTime, distTime)
  // Fresh by construction unless a test ages it: `new Date()` would race the
  // source file written microseconds earlier on a coarse-granularity clock.
  const bridgeTime = over.bridgeDistMtime ?? new Date(Date.now() + 5000)
  utimesSync(join(root, 'omp-auth/dist/server.js'), bridgeTime, bridgeTime)
  utimesSync(join(root, 'omp-auth/src/server.ts'), new Date(Date.now() - 5000), new Date(Date.now() - 5000))
  return root
}

/** The argv `compose()` builds for the main dev stack — the key `plant` needs
 *  to answer one specific `up`. */
const devCompose = (root: string, ...op: string[]) => [
  'compose',
  '-f',
  join(root, 'docker/sidecars.compose.yml'),
  '-f',
  join(root, 'docker/dev-compose.yml'),
  ...op,
]

const plantInfra = (ctx: FakeCtx) => {
  ctx.plant(['docker', ['exec', 'talaria-postgres-dev', 'pg_isready', '-U', 'talaria', '-d', 'talaria']], '')
  ctx.plant(['docker', ['exec', 'talaria-redis-dev', 'redis-cli', 'ping']], '')
}

/** Run dev, capturing the die() message instead of failing the test on it. */
const attempt = async (ctx: FakeCtx): Promise<string> => {
  try {
    await runDev(ctx)
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

describe('talaria dev — mcp staleness', () => {
  test('fresh dist → no rebuild', async () => {
    const root = makeTree({ distMtime: new Date(Date.now() + 5000) })
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(
      ctx.calls.some((c) => c.cmd === 'bun' && c.args[1] === 'build' && c.opts?.cwd === join(root, 'mcp')),
    ).toBe(false)
  })

  test('one deep src file newer than dist → rebuild', async () => {
    const root = makeTree()
    const later = new Date(Date.now() + 10_000)
    utimesSync(join(root, 'mcp/src/deep/util.ts'), later, later)
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(ctx.calls.some((c) => c.cmd === 'bun' && c.args[1] === 'build' && c.opts?.cwd === join(root, 'mcp'))).toBe(true)
  })

  // MISSING AND STALE ARE DIFFERENT FAILURES. A dist that exists still SERVES
  // the previous build, so a failed rebuild is a warning and the stack comes
  // up; a dist that is absent serves nothing, so a stack that came up anyway
  // would hand every agent an empty tool list.
  test('a stale dist that will not rebuild warns and dev carries on', async () => {
    const root = makeTree({ distMtime: new Date(0) })
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    ctx.plant(['bun', ['run', 'build']], new Error('tsc blew up'), join(root, 'mcp'))
    const died = await attempt(ctx)
    expect(died).toBe('')
    expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes('mcp/dist is STALE'))).toBe(true)
  })

  test('a missing dist that will not rebuild stops the stack', async () => {
    const root = makeTree({ distMtime: new Date(0) })
    rmSync(join(root, 'mcp/dist/index.js'))
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    ctx.plant(['bun', ['run', 'build']], new Error('tsc blew up'), join(root, 'mcp'))
    const died = await attempt(ctx)
    expect(died).toContain('mcp/dist does not exist')
    expect(died).toContain('serve NOTHING')
  })

  test('TALARIA_SKIP_MCP_BUILD=1 → never builds', async () => {
    const root = makeTree({ distMtime: new Date(0) })
    const ctx = fakeCtx({ env: { TALARIA_SKIP_MCP_BUILD: '1' } })
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(ctx.calls.some((c) => c.args[1] === 'build' && c.opts?.cwd === join(root, 'mcp'))).toBe(false)
  })
})

describe('talaria dev — omp auth bridge', () => {
  test('fresh bundle → no rebuild', async () => {
    const root = makeTree()
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(
      ctx.calls.some((c) => c.cmd === 'bun' && c.args[1] === 'build' && c.opts?.cwd === join(root, 'omp-auth')),
    ).toBe(false)
  })

  test('src newer than the bundle → rebuild', async () => {
    const root = makeTree({ bridgeDistMtime: new Date(0) })
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(
      ctx.calls.some((c) => c.cmd === 'bun' && c.args[1] === 'build' && c.opts?.cwd === join(root, 'omp-auth')),
    ).toBe(true)
  })

  test('TALARIA_SKIP_OMP_AUTH_BUILD=1 → never builds', async () => {
    const root = makeTree({ bridgeDistMtime: new Date(0) })
    const ctx = fakeCtx({ env: { TALARIA_SKIP_OMP_AUTH_BUILD: '1' } })
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(ctx.calls.some((c) => c.args[1] === 'build' && c.opts?.cwd === join(root, 'omp-auth'))).toBe(false)
  })

  // A bundle that will not build must not stop a dev stack: coding accounts
  // are opt-in behind an admin toggle, and everything else is unaffected.
  test('a failed build warns and dev carries on', async () => {
    const root = makeTree({ bridgeDistMtime: new Date(0) })
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    // SCOPED TO THE BRIDGE. This used to plant the failure for any
    // `bun run build` and lean on "mcp's dist is fresh in this tree" to keep
    // the toolkit's build out of the way — but freshness here is an mtime
    // relationship, and a CI checkout gives mcp/src and mcp/dist the same
    // checkout time, so the toolkit built too, hit this planted failure, and
    // died before the bridge was ever reached. The cwd says which build.
    ctx.plant(['bun', ['run', 'build']], new Error('bundler blew up'), join(root, 'omp-auth'))
    await runDev(ctx)
    expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes('omp-auth/ failed to build'))).toBe(true)
  })
})

describe('talaria dev — gates', () => {
  test('TALARIA_DEVBOX set → zero docker invocations, straight to the app', async () => {
    const root = makeTree({ uiEnv: 'TALARIA_DEVBOX=demo\nDATABASE_URL=x\n' })
    const ctx = fakeCtx({ env: { TALARIA_DEVBOX: 'demo' } })
    ctx.root = root
    await runDev(ctx)
    expect(ctx.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)
    expect(ctx.calls.some((c) => c.cmd === 'bun' && c.args[1] === 'dev' && c.opts?.cwd === join(root, 'ui'))).toBe(true)
  })

  test('linked worktree without TALARIA_WORKTREE stamp → refuses before any exec', async () => {
    const root = makeTree()
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    const ctx = fakeCtx()
    ctx.root = root
    const msg = await attempt(ctx)
    expect(msg).toContain('worktree without an isolated stack')
    expect(ctx.calls.length).toBe(0)
  })

  test('linked worktree WITH the stamp proceeds', async () => {
    const root = makeTree({ uiEnv: 'TALARIA_WORKTREE=wt\nDATABASE_URL=x\nREDIS_URL=y\nTALARIA_FLEET_PROJECT=talaria-wt-wt-fleet\n' })
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(ctx.calls.some((c) => c.cmd === 'docker' && c.args[0] === 'compose')).toBe(true)
  })

  test('an unpullable storage sidecar warns — it never takes postgres and redis down with it', async () => {
    // A single `up` resolves every image before it creates any container, so
    // one unpullable sidecar image used to abort the two services the app
    // cannot boot without. Its own `up`, its own warning.
    const root = makeTree()
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    ctx.plant(['docker', devCompose(root, 'up', '-d', 'storage')], new Error('denied'))
    await runDev(ctx)
    const ups = ctx.calls.filter((c) => c.args.includes('up'))
    expect(ups[0]!.args.slice(-3)).toEqual(['postgres', 'redis', 'qdrant'])
    expect(ups.some((c) => c.args.at(-1) === 'storage')).toBe(true)
    expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes('object storage failed'))).toBe(true)
    // …and the run reached the app, which is the whole point
    expect(ctx.calls.some((c) => c.cmd === 'bun' && c.args[0] === 'run' && c.args[1] === 'dev')).toBe(true)
  })

  test('worktree mode aims compose at its own project and probes its own containers', async () => {
    const root = makeTree({
      uiEnv:
        'TALARIA_WORKTREE=demo\nDATABASE_URL=postgres://talaria:talaria@127.0.0.1:5601/talaria\nREDIS_URL=redis://127.0.0.1:6501\nPORT=5301\n' +
        'TALARIA_FLEET_PROJECT=talaria-wt-demo-fleet\n',
    })
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', ['exec', 'talaria-pg-demo', 'pg_isready', '-U', 'talaria', '-d', 'talaria']], '')
    ctx.plant(['docker', ['exec', 'talaria-redis-demo', 'redis-cli', 'ping']], '')
    await runDev(ctx)
    // exactly one compose up — the worktree project, scoped to its two services
    const ups = ctx.calls.filter((c) => c.args.includes('up'))
    expect(ups).toHaveLength(1)
    expect(ups[0].args).toContain('talaria-wt-demo')
    expect(ups[0].args.slice(-2)).toEqual(['postgres', 'redis'])
    // ports lifted from the URLs `talaria worktree` wrote
    expect(ctx.env.TALARIA_PG_PORT).toBe('5601')
    expect(ctx.env.TALARIA_REDIS_PORT).toBe('6501')
    // readiness probes hit the worktree's containers — never main's, and no
    // sidecar is brought up (they belong to the main stack)
    expect(ctx.calls.some((c) => c.args[0] === 'exec' && c.args[1] === 'talaria-pg-demo')).toBe(true)
    expect(ctx.calls.some((c) => c.args.includes('talaria-postgres-dev'))).toBe(false)
    expect(ctx.calls.some((c) => c.args.includes('searxng'))).toBe(false)
  })

  test('a worktree\'s own PORT reaches vite (vite\'s script hardcodes 5273)', async () => {
    const root = makeTree({ uiEnv: 'TALARIA_WORKTREE=wt\nDATABASE_URL=x\nREDIS_URL=y\nPORT=5310\nTALARIA_FLEET_PROJECT=talaria-wt-wt-fleet\n' })
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    const dev = ctx.calls.find((c) => c.cmd === 'bun' && c.args[0] === 'run' && c.args[1] === 'dev')!
    expect(dev.args).toEqual(['run', 'dev', '--', '--port', '5310'])
  })

  test('the primary checkout does NOT get a port flag', async () => {
    const root = makeTree({ uiEnv: 'DATABASE_URL=x\nREDIS_URL=y\nPORT=5273\n' })
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    const dev = ctx.calls.find((c) => c.cmd === 'bun' && c.args[0] === 'run' && c.args[1] === 'dev')!
    expect(dev.args).toEqual(['run', 'dev'])
  })

  test("a shell TALARIA_PG_CONTAINER naming main's container is refused in a worktree, not obeyed", async () => {
    // It used to win (`??=`): the worktree's `up` would then try to create
    // main's container name, and the readiness probes would answer for main.
    const root = makeTree({ uiEnv: 'TALARIA_WORKTREE=demo\nDATABASE_URL=x\nREDIS_URL=y\n' })
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    const ctx = fakeCtx({ env: { TALARIA_PG_CONTAINER: 'talaria-postgres-dev' } })
    ctx.root = root
    const msg = await attempt(ctx)
    expect(msg).toContain('TALARIA_PG_CONTAINER=talaria-postgres-dev')
    expect(ctx.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)
  })

  describe("a worktree's own ui/.env lines beat nothing in the shell: a different value is refused", () => {
    // apiEnv and vite both let the shell win. A stale export from main's
    // shell made this worktree's api a second server on main's database.
    const WT_ENV =
      'TALARIA_WORKTREE=demo\nDATABASE_URL=postgres://talaria:talaria@127.0.0.1:5601/talaria\nREDIS_URL=redis://127.0.0.1:6501\n' +
      'PORT=5301\nTALARIA_API_PORT=5401\nTALARIA_FLEET_PROJECT=talaria-wt-demo-fleet\n'
    const wtTree = (uiEnv = WT_ENV) => {
      const root = makeTree({ uiEnv })
      writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
      return root
    }
    const plantWt = (ctx: FakeCtx) => {
      ctx.plant(['docker', ['exec', 'talaria-pg-demo', 'pg_isready', '-U', 'talaria', '-d', 'talaria']], '')
      ctx.plant(['docker', ['exec', 'talaria-redis-demo', 'redis-cli', 'ping']], '')
    }

    for (const [key, mains] of [
      ['DATABASE_URL', 'postgres://talaria:talaria@127.0.0.1:5544/talaria'],
      ['REDIS_URL', 'redis://127.0.0.1:6399'],
      ['TALARIA_FLEET_PROJECT', 'talaria-fleet'],
      ['TALARIA_API_PORT', '5274'],
      ['PORT', '5273'],
    ] as const) {
      test(`${key} naming main's is refused before any docker call or api spawn`, async () => {
        const ctx = fakeCtx({ env: { [key]: mains } })
        ctx.root = wtTree()
        const msg = await attempt(ctx)
        expect(msg).toContain('worktree "demo"')
        expect(msg).toContain(`${key}=`)
        expect(msg).toContain(`unset ${key}`)
        expect(msg).not.toContain('talaria:talaria@')
        expect(ctx.calls.filter((c) => c.cmd === 'docker' || c.cmd === 'cargo' || c.cmd === 'bun')).toHaveLength(0)
      })
    }

    test('the message names each clash with both values, credentials cut', async () => {
      const ctx = fakeCtx({ env: { DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5544/talaria', REDIS_URL: 'redis://127.0.0.1:6399' } })
      ctx.root = wtTree()
      const msg = await attempt(ctx)
      expect(msg).toContain('DATABASE_URL=postgres://127.0.0.1:5544/talaria   (ui/.env: postgres://127.0.0.1:5601/talaria)')
      expect(msg).toContain('REDIS_URL=redis://127.0.0.1:6399   (ui/.env: redis://127.0.0.1:6501)')
      expect(msg).toContain('unset DATABASE_URL REDIS_URL')
    })

    test("a shell value equal to ui/.env's is no conflict, and the stack comes up", async () => {
      const ctx = fakeCtx({
        env: { DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5601/talaria', TALARIA_FLEET_PROJECT: 'talaria-wt-demo-fleet', TALARIA_API: 'off' },
      })
      ctx.root = wtTree()
      plantWt(ctx)
      await runDev(ctx)
      expect(ctx.calls.some((c) => c.args.includes('talaria-wt-demo') && c.args.includes('up'))).toBe(true)
    })

    test('a line ui/.env does not carry is left to the shell, as before', async () => {
      const ctx = fakeCtx({ env: { TALARIA_MCP_PORT: '5999', TALARIA_API: 'off' } })
      ctx.root = wtTree()
      plantWt(ctx)
      await runDev(ctx)
      expect(ctx.calls.some((c) => c.args.includes('talaria-wt-demo') && c.args.includes('up'))).toBe(true)
    })

    test('no TALARIA_FLEET_PROJECT: refused, since its agents would render into main\'s fleet project', async () => {
      const ctx = fakeCtx()
      ctx.root = wtTree(WT_ENV.replace('TALARIA_FLEET_PROJECT=talaria-wt-demo-fleet\n', ''))
      const msg = await attempt(ctx)
      expect(msg).toContain('names no TALARIA_FLEET_PROJECT')
      expect(msg).toContain('TALARIA_FLEET_PROJECT=talaria-wt-demo-fleet')
      expect(ctx.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)
    })

    test("a fleet project in ui/.env that is not the worktree's own is refused", async () => {
      const ctx = fakeCtx()
      ctx.root = wtTree(WT_ENV.replace('talaria-wt-demo-fleet', 'talaria-fleet'))
      expect(await attempt(ctx)).toContain("is not worktree \"demo\"'s fleet project")
      expect(ctx.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)
    })

    test("a shell TALARIA_PG_PORT that is not DATABASE_URL's is refused; the URL's port is what compose gets", async () => {
      const bad = fakeCtx({ env: { TALARIA_PG_PORT: '5544' } })
      bad.root = wtTree()
      const msg = await attempt(bad)
      expect(msg).toContain('TALARIA_PG_PORT=5544')
      expect(msg).toContain('port 5601')
      expect(bad.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)
      const ok = fakeCtx({ env: { TALARIA_API: 'off' } })
      ok.root = wtTree()
      plantWt(ok)
      await runDev(ok)
      expect(ok.env.TALARIA_PG_PORT).toBe('5601')
      expect(ok.env.TALARIA_REDIS_PORT).toBe('6501')
    })

    test('the primary keeps the shell as its override point', async () => {
      const ctx = fakeCtx({ env: { DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5999/talaria', TALARIA_API: 'off' } })
      ctx.root = makeTree({ uiEnv: 'DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\nREDIS_URL=y\n' })
      plantInfra(ctx)
      await runDev(ctx)
      expect(ctx.calls.some((c) => c.cmd === 'bun' && c.args[0] === 'run' && c.args[1] === 'dev')).toBe(true)
    })
  })

  test("the shim run from inside another checkout refuses: it would start the shim's own stack", async () => {
    // The installed `talaria` shim always runs the primary's CLI, so its
    // root is the primary; standing in a worktree, `talaria dev` brought up
    // the PRIMARY's stack.
    const primary = makeTree()
    mkdirSync(join(primary, '.git'))
    mkdirSync(join(primary, 'cli/bin'), { recursive: true })
    writeFileSync(join(primary, 'cli/bin/talaria.ts'), '')
    const wt = makeTree({ uiEnv: 'TALARIA_WORKTREE=demo\nDATABASE_URL=x\nREDIS_URL=y\n' })
    writeFileSync(join(wt, '.git'), 'gitdir: /elsewhere\n')
    mkdirSync(join(wt, 'cli/bin'), { recursive: true })
    writeFileSync(join(wt, 'cli/bin/talaria.ts'), '')
    const ctx = fakeCtx({ cwd: wt })
    ctx.root = primary
    const msg = await attempt(ctx)
    expect(msg).toContain(`cd ${wt} && bun talaria dev`)
    expect(ctx.calls).toHaveLength(0)
  })

  test("the primary refuses an inherited worktree's container names before any docker call; its own override passes", async () => {
    // A worktree's `talaria dev` sets TALARIA_PG_CONTAINER (and the ports) in
    // its own process. From a shell under it, the primary's `up` would
    // interpolate that name into main's project: compose recreates main's
    // postgres under it, or removes it on the name conflict.
    const root = makeTree({ uiEnv: 'DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\nREDIS_URL=y\n' })
    const ctx = fakeCtx({
      env: { TALARIA_PG_CONTAINER: 'talaria-pg-demo', TALARIA_PG_PORT: '5605', TALARIA_REDIS_CONTAINER: 'talaria-redis-demo', TALARIA_API: 'off' },
    })
    ctx.root = root
    const msg = await attempt(ctx)
    expect(msg).toContain('TALARIA_PG_CONTAINER=talaria-pg-demo')
    expect(msg).toContain('TALARIA_REDIS_CONTAINER=talaria-redis-demo')
    expect(msg).toContain('env -u TALARIA_PG_CONTAINER -u TALARIA_REDIS_CONTAINER -u TALARIA_PG_PORT bun talaria dev')
    expect(ctx.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)

    const own = fakeCtx({ env: { TALARIA_PG_CONTAINER: 'my-main-pg', TALARIA_API: 'off' } })
    own.root = root
    own.plant(['docker', ['exec', 'my-main-pg', 'pg_isready', '-U', 'talaria', '-d', 'talaria']], '')
    own.plant(['docker', ['exec', 'talaria-redis-dev', 'redis-cli', 'ping']], '')
    await runDev(own)
    expect(own.calls.some((c) => c.args[0] === 'exec' && c.args[1] === 'my-main-pg')).toBe(true)
  })

  test("the primary refuses a shell that inherited a worktree's whole env, and the command it prints is complete", async () => {
    // A worktree's `talaria dev` exports its container names and ports; its
    // vite loads its ui/.env into process.env, and its api gets the same
    // lines through apiEnv. A shell under any of them carries all of it, and
    // the primary's `talaria dev` lets the shell win (apiEnv, vite's loader).
    // Unsetting only the container names would start main's app and api on
    // the worktree's database, fleet project and api port.
    const INHERITED: Record<string, string> = {
      TALARIA_PG_CONTAINER: 'talaria-pg-demo',
      TALARIA_PG_PORT: '5605',
      TALARIA_REDIS_CONTAINER: 'talaria-redis-demo',
      TALARIA_REDIS_PORT: '6505',
      TALARIA_WORKTREE: 'demo',
      DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5605/talaria',
      REDIS_URL: 'redis://127.0.0.1:6505',
      PORT: '5305',
      TALARIA_API_PORT: '5405',
      TALARIA_RUST_API_URL: 'http://127.0.0.1:5405',
      TALARIA_GATEWAY_SELF_URL: 'http://host.docker.internal:5305/api/llm/v1',
      TALARIA_FLEET_PROJECT: 'talaria-wt-demo-fleet',
      TALARIA_MCP_PORT: '5285',
      TALARIA_OMP_AUTH_PORT: '5277',
      TALARIA_GATEWAY_PORT_BASE: '8905',
    }
    const root = makeTree({ uiEnv: 'DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\nREDIS_URL=redis://127.0.0.1:6379\n' })
    const ctx = fakeCtx({ env: { ...INHERITED, TALARIA_API: 'off', HOME: '/home/someone' } })
    ctx.root = root
    const msg = await attempt(ctx)
    expect(msg).toContain('TALARIA_WORKTREE=demo in the environment')
    expect(ctx.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)
    const hint = /env ((?:-u \S+ )+)bun talaria dev/.exec(msg)
    expect(hint).not.toBeNull()
    const unset = hint![1]!.trim().split(/\s+/).filter((w) => w !== '-u')
    expect(unset.sort()).toEqual(Object.keys(INHERITED).sort())

    // Run what it printed: nothing of the worktree is left for compose, the
    // probes, vite or the api to read, and the boot reaches main's stack.
    const env: Record<string, string> = { TALARIA_API: 'off', HOME: '/home/someone' }
    for (const [k, v] of Object.entries({ ...INHERITED })) if (!unset.includes(k)) env[k] = v
    const rerun = fakeCtx({ env })
    rerun.root = root
    plantInfra(rerun)
    await runDev(rerun)
    expect(Object.values(rerun.env).some((v) => v?.includes('demo') || v?.includes('5605'))).toBe(false)
    expect(rerun.calls.some((c) => c.args.some((a) => a.includes('demo')))).toBe(false)
    expect(rerun.calls.some((c) => c.args[0] === 'exec' && c.args[1] === 'talaria-postgres-dev')).toBe(true)
    expect(rerun.calls.some((c) => c.cmd === 'bun' && c.args[1] === 'dev' && c.opts?.cwd === join(root, 'ui'))).toBe(true)
  })

  test("the primary refuses a shell TALARIA_WORKTREE or a worktree's fleet project on its own, before any docker call", async () => {
    const root = makeTree({ uiEnv: 'DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\nREDIS_URL=y\n' })
    const marked = fakeCtx({ env: { TALARIA_WORKTREE: 'demo', DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5605/talaria', TALARIA_API: 'off' } })
    marked.root = root
    expect(await attempt(marked)).toContain('env -u TALARIA_WORKTREE -u DATABASE_URL bun talaria dev')
    expect(marked.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)

    const fleet = fakeCtx({ env: { TALARIA_FLEET_PROJECT: 'talaria-wt-demo-fleet', TALARIA_API: 'off' } })
    fleet.root = root
    const msg = await attempt(fleet)
    expect(msg).toContain("talaria-wt-demo-fleet is a worktree's fleet project")
    expect(msg).toContain('`talaria dev` would start its api rendering agents there')
    expect(fleet.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)
  })

  test('a ui/.env that marks a devbox, with no TALARIA_DEVBOX in the shell, is refused rather than run as the primary', async () => {
    // The box branch reads the shell only; without this, the primary path
    // brought up main's dev compose from a box's checkout.
    const root = makeTree({ uiEnv: 'TALARIA_DEVBOX=b1\nDATABASE_URL=postgres://talaria:talaria@postgres:5432/talaria\n' })
    const ctx = fakeCtx({ env: { TALARIA_API: 'off' } })
    ctx.root = root
    expect(await attempt(ctx)).toContain('this checkout is devbox "b1"')
    expect(ctx.calls.filter((c) => c.cmd === 'docker')).toHaveLength(0)
  })

  test("an empty TALARIA_PG_CONTAINER in the primary probes main's default, the container compose creates", async () => {
    // dev-compose.yml reads `${TALARIA_PG_CONTAINER:-talaria-postgres-dev}`:
    // empty is the default. `?? PG_CONTAINER` probed a container named ''.
    const root = makeTree()
    const ctx = fakeCtx({ env: { TALARIA_PG_CONTAINER: '', TALARIA_REDIS_CONTAINER: '', TALARIA_API: 'off' } })
    ctx.root = root
    plantInfra(ctx)
    expect(await attempt(ctx)).toBe('')
    expect(ctx.calls.some((c) => c.args[0] === 'exec' && c.args[1] === 'talaria-postgres-dev')).toBe(true)
    expect(ctx.calls.some((c) => c.args[0] === 'exec' && c.args[1] === 'talaria-redis-dev')).toBe(true)
    expect(ctx.calls.some((c) => c.args[0] === 'exec' && c.args[1] === '')).toBe(false)
  })

  test('missing ui/.env dies pointing at setup', async () => {
    const ctx = fakeCtx()
    ctx.root = mkdtempSync(join(tmpdir(), 'talaria-empty-'))
    const msg = await attempt(ctx)
    expect(msg).toContain('bun talaria setup')
  })
})

describe('talaria dev — s3 lift', () => {
  test('creds from ui/.env reach the compose environment', async () => {
    const root = makeTree({ uiEnv: 'DATABASE_URL=x\nREDIS_URL=y\nTALARIA_S3_ACCESS_KEY=lifted\nTALARIA_S3_SECRET_KEY=sekret\n' })
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(ctx.env.TALARIA_S3_ACCESS_KEY).toBe('lifted')
    expect(ctx.env.TALARIA_S3_SECRET_KEY).toBe('sekret')
  })

  test('an exported value is not overwritten by the file', async () => {
    const root = makeTree({ uiEnv: 'DATABASE_URL=x\nREDIS_URL=y\nTALARIA_S3_ACCESS_KEY=fromfile\n' })
    const ctx = fakeCtx({ env: { TALARIA_S3_ACCESS_KEY: 'fromenv' } })
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(ctx.env.TALARIA_S3_ACCESS_KEY).toBe('fromenv')
  })
})

// The Rust api sidecar, exercised directly (runDev tests above plant execs, but
// rustApi's probe is a real HTTP fetch and its spawn is a real process — so
// these cases stick to the three paths that need neither: gate-off, no-cargo,
// adopt). Real sockets on ephemeral ports: the gate is read from ctx.env, but
// the PORT default of 5274 could be a live sidecar on a dev machine, so every
// case pins TALARIA_API_PORT to a port it owns.
describe('talaria dev — rust api sidecar', () => {
  /** An ephemeral listener answering 200 on loopback; hands its port to `fn`,
   *  then closes. */
  const withListener = async (fn: (port: number) => Promise<void>) => {
    const srv = createServer((_req, res) => {
      res.statusCode = 200
      res.end('ok')
    })
    const port = await new Promise<number>((resolve) =>
      srv.listen(0, '127.0.0.1', () => resolve((srv.address() as AddressInfo).port)),
    )
    try {
      await fn(port)
    } finally {
      await new Promise((r) => srv.close(r))
    }
  }

  /** A port that was just freed — closed for the probe, with none of the
   *  flakiness of guessing an unused fixed one. */
  const closedPort = async (): Promise<number> => {
    const srv = createServer()
    const port = await new Promise<number>((resolve) =>
      srv.listen(0, '127.0.0.1', () => resolve((srv.address() as AddressInfo).port)),
    )
    await new Promise((r) => srv.close(r))
    return port
  }

  test("opt-out only — TALARIA_API=off touches nothing (unset is the default-ON posture)", async () => {
    const ctx = fakeCtx({ env: { TALARIA_API: 'off' } })
    await rustApi(ctx, 'DATABASE_URL=x\n')
    expect(ctx.calls).toHaveLength(0)
    expect(ctx.logLines).toHaveLength(0)
  })

  test('no cargo on PATH (with TALARIA_API unset — the default) → warn and skip, API-DARK at full volume', async () => {
    const port = await closedPort()
    const ctx = fakeCtx({ env: { TALARIA_API_PORT: String(port) } })
    ctx.plant(['cargo', ['--version']], new Error('not found'))
    await rustApi(ctx, 'DATABASE_URL=x\n')
    expect(ctx.logLines.filter((l) => l.kind === 'warn').map((l) => l.msg)).toEqual([
      'no cargo on PATH — skipping the Rust api. The UI will serve, but every /api/* request fails:' +
        ' the api is the whole product surface, not a side feature.',
    ])
    // the hop URL lifted even though the api never came up — the proxy aims
    // at where the api WOULD be, and its 502 names reality
    expect(ctx.env.TALARIA_RUST_API_URL).toBe(`http://127.0.0.1:${port}`)
    // looked for cargo, but never tried to run it
    expect(ctx.calls.map((c) => [c.cmd, ...c.args])).toEqual([['cargo', '--version']])
  })

  test('something already answering on the port → adopt, never check cargo, lift the hop URL', async () => {
    await withListener(async (port) => {
      const ctx = fakeCtx({ env: { TALARIA_API_PORT: String(port) } })
      await rustApi(ctx, 'DATABASE_URL=x\n')
      expect(ctx.logLines).toHaveLength(1)
      expect(ctx.logLines[0]).toMatchObject({ kind: 'say', msg: `rust api → already listening on :${port}; adopting it` })
      // lifted even on adopt — ui/.env here names no URL
      expect(ctx.env.TALARIA_RUST_API_URL).toBe(`http://127.0.0.1:${port}`)
      expect(ctx.calls).toHaveLength(0)
    })
  })

  test('an explicit URL (shell or ui/.env) is never overwritten by the lift', async () => {
    await withListener(async (port) => {
      const ctx = fakeCtx({ env: { TALARIA_API_PORT: String(port), TALARIA_RUST_API_URL: 'http://10.0.0.5:5274' } })
      await rustApi(ctx, 'DATABASE_URL=x\n')
      expect(ctx.env.TALARIA_RUST_API_URL).toBe('http://10.0.0.5:5274')
    })
  })

  test("ui/.env's TALARIA_API_PORT is read — a worktree's api port lives in that file", async () => {
    await withListener(async (port) => {
      // Nothing in the shell: the port comes from the file, which is where
      // `talaria worktree` writes it. Reading only ctx.env meant a worktree's
      // own api port was ignored and every stack fell back to the shared
      // :5274 — where `talaria dev` ADOPTS whatever is already listening, so
      // the second worktree proxied /api/* to the first one's database.
      const ctx = fakeCtx()
      await rustApi(ctx, `DATABASE_URL=x\nTALARIA_API_PORT=${port}\n`)
      expect(ctx.logLines[0]).toMatchObject({
        kind: 'say',
        msg: `rust api → already listening on :${port}; adopting it`,
      })
      expect(ctx.env.TALARIA_RUST_API_URL).toBe(`http://127.0.0.1:${port}`)
    })
  })

  test('the shell still wins over ui/.env for the api port', async () => {
    await withListener(async (port) => {
      // Same precedence as the S3 lift: the environment is the override point.
      const ctx = fakeCtx({ env: { TALARIA_API_PORT: String(port) } })
      await rustApi(ctx, 'DATABASE_URL=x\nTALARIA_API_PORT=59999\n')
      expect(ctx.env.TALARIA_RUST_API_URL).toBe(`http://127.0.0.1:${port}`)
    })
  })

  test('the api port is lifted into the cargo child — the api BINDS it', async () => {
    const port = await closedPort()
    // cargo is missing, so the spawn never happens and this stops at the warn —
    // but the lift list is what matters, and it is read before that. The api
    // reads TALARIA_API_PORT for its own bind (talaria-config, default 5274),
    // so a port the proxy dials but the child never sees would leave the api
    // bound to the shared port while the app looked at the private one.
    const ctx = fakeCtx()
    ctx.plant(['cargo', ['--version']], new Error('not found'))
    await rustApi(ctx, `DATABASE_URL=x\nTALARIA_API_PORT=${port}\n`)
    expect(ctx.env.TALARIA_RUST_API_URL).toBe(`http://127.0.0.1:${port}`)
  })
})

// What the cargo child is handed. The api names every container it owns after
// the instance (`talaria-appdb-<instance>-<slug>`, `talaria-mcp-<instance>-…`),
// and the app side reads that instance from ui/.env — so the two have to agree.
describe('talaria dev — the env the rust api gets', () => {
  const uiEnv = [
    'DATABASE_URL=postgres://talaria:pw@127.0.0.1:5605/talaria',
    'REDIS_URL=redis://127.0.0.1:6505',
    'TALARIA_API_PORT=5405',
    'PORT=5305',
    'TALARIA_WORKTREE=apps-enable',
    'TALARIA_RUST_API_URL=http://127.0.0.1:5405',
    'UNRELATED=1',
  ].join('\n')

  test("a worktree's name reaches the api, so it names the same app database container", () => {
    const env = apiEnv({}, uiEnv)
    expect(env.TALARIA_WORKTREE).toBe('apps-enable')
    expect(env.TALARIA_API_PORT).toBe('5405')
    expect(env.DATABASE_URL).toBe('postgres://talaria:pw@127.0.0.1:5605/talaria')
    expect(env.UNRELATED).toBeUndefined()
  })

  test('the shell wins over ui/.env', () => {
    expect(apiEnv({ TALARIA_WORKTREE: 'from-shell' }, uiEnv).TALARIA_WORKTREE).toBe('from-shell')
  })

  test("a worktree's toolkit, omp-auth and gateway ports reach the api that spawns and renders them", () => {
    const env = apiEnv(
      {},
      `${uiEnv}\nTALARIA_MCP_PORT=5705\nTALARIA_OMP_AUTH_PORT=5805\nTALARIA_GATEWAY_PORT_BASE=20500\n`,
    )
    expect(env.TALARIA_MCP_PORT).toBe('5705')
    expect(env.TALARIA_OMP_AUTH_PORT).toBe('5805')
    expect(env.TALARIA_GATEWAY_PORT_BASE).toBe('20500')
    // main's checkout names none, and the api keeps its defaults
    const bare = apiEnv({}, 'DATABASE_URL=x\n')
    for (const k of ['TALARIA_MCP_PORT', 'TALARIA_OMP_AUTH_PORT', 'TALARIA_GATEWAY_PORT_BASE']) {
      expect(k in bare).toBe(false)
    }
  })

  test('the main checkout (no worktree name anywhere) passes none', () => {
    expect('TALARIA_WORKTREE' in apiEnv({}, 'DATABASE_URL=x\n')).toBe(false)
  })

  test("ui/.env's opt-in to local app sources reaches the api, which is what reads it", () => {
    expect(apiEnv({}, `${uiEnv}\nTALARIA_APPS_LOCAL_SOURCES=1`).TALARIA_APPS_LOCAL_SOURCES).toBe('1')
    expect('TALARIA_APPS_LOCAL_SOURCES' in apiEnv({}, uiEnv)).toBe(false)
  })
})
