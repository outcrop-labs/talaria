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
import { apiEnv, portHolder, runDev, rustApi, viteArgs } from './dev'
import { WORKTREE_COMPOSE, pgReadyArgs } from '../compose'
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
  ctx.plant(['docker', pgReadyArgs('talaria-postgres-dev')], '')
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
    const root = makeTree({ uiEnv: 'TALARIA_WORKTREE=wt\nDATABASE_URL=x\nREDIS_URL=y\n' })
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
        'TALARIA_WORKTREE=demo\nDATABASE_URL=postgres://talaria:talaria@127.0.0.1:5601/talaria\nREDIS_URL=redis://127.0.0.1:6501\nPORT=5301\n',
    })
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', pgReadyArgs('talaria-pg-demo')], '')
    ctx.plant(['docker', ['exec', 'talaria-redis-demo', 'redis-cli', 'ping']], '')
    await runDev(ctx)
    // exactly one compose up — the worktree project, scoped to its two services
    const ups = ctx.calls.filter((c) => c.args.includes('up'))
    expect(ups).toHaveLength(1)
    expect(ups[0].args).toContain('talaria-wt-demo')
    expect(ups[0].args.slice(-2)).toEqual(['postgres', 'redis'])
    // the same three files `talaria worktree` brought the stack up with —
    // without the last one, compose would recreate both on a project network
    expect(ups[0].args).toContain(join(root, WORKTREE_COMPOSE))
    // ports lifted from the URLs `talaria worktree` wrote
    expect(ctx.env.TALARIA_PG_PORT).toBe('5601')
    expect(ctx.env.TALARIA_REDIS_PORT).toBe('6501')
    // readiness probes hit the worktree's containers — never main's, and no
    // sidecar is brought up (they belong to the main stack)
    expect(ctx.calls.some((c) => c.args[0] === 'exec' && c.args[1] === 'talaria-pg-demo')).toBe(true)
    expect(ctx.calls.some((c) => c.args.includes('talaria-postgres-dev'))).toBe(false)
    expect(ctx.calls.some((c) => c.args.includes('searxng'))).toBe(false)
  })

  test('a worktree\'s own PORT reaches vite (vite\'s script hardcodes 5273), bound strictly', async () => {
    const root = makeTree({ uiEnv: 'TALARIA_WORKTREE=wt\nDATABASE_URL=x\nREDIS_URL=y\nPORT=5310\n' })
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    const dev = ctx.calls.find((c) => c.cmd === 'bun' && c.args[0] === 'run' && c.args[1] === 'dev')!
    // --strictPort: a busy port used to move vite to the NEXT one while
    // TALARIA_GATEWAY_SELF_URL and the rest of ui/.env kept naming 5310.
    expect(dev.args).toEqual(['run', 'dev', '--', '--port', '5310', '--strictPort'])
    // ...and the port was asked about before anything started
    const probe = ctx.calls.findIndex((c) => c.cmd === 'ss')
    expect(ctx.calls[probe]!.args).toEqual(['-Hltnp', 'sport', '=', ':5310'])
    expect(probe).toBeLessThan(ctx.calls.findIndex((c) => c.args.includes('up')))
  })

  test('the primary checkout keeps the script\'s 5273, strictly', async () => {
    const root = makeTree({ uiEnv: 'DATABASE_URL=x\nREDIS_URL=y\nPORT=5273\n' })
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    const dev = ctx.calls.find((c) => c.cmd === 'bun' && c.args[0] === 'run' && c.args[1] === 'dev')!
    expect(dev.args).toEqual(['run', 'dev', '--', '--strictPort'])
    expect(ctx.calls.find((c) => c.cmd === 'ss')!.args).toEqual(['-Hltnp', 'sport', '=', ':5273'])
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

describe('talaria dev — a worktree gives back its old project network', () => {
  // A worktree cut before docker/worktree.compose.yml had its two containers
  // on `talaria-wt-<name>_default`. The `up` moves them to the default bridge;
  // the network stays behind holding a subnet, and the sweep will not take it
  // while the worktree lives.
  const NET = 'talaria-wt-demo_default'
  const inspect = ['network', 'inspect', '--format', '{{.Id}}', NET]
  const ps = ['ps', '-a', '--format', '{{.Networks}}']
  const rm = ['network', 'rm', NET]
  const same = (a: string[], b: string[]) => JSON.stringify(a) === JSON.stringify(b)
  const called = (ctx: FakeCtx, args: string[]) => ctx.calls.some((c) => c.cmd === 'docker' && same(c.args, args))
  const reachedApp = (ctx: FakeCtx) => ctx.calls.some((c) => c.cmd === 'bun' && c.args[0] === 'run' && c.args[1] === 'dev')
  const worktreeCtx = () => {
    const root = makeTree({
      uiEnv: 'TALARIA_WORKTREE=demo\nDATABASE_URL=postgres://talaria:talaria@127.0.0.1:5601/talaria\nREDIS_URL=redis://127.0.0.1:6501\n',
    })
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', pgReadyArgs('talaria-pg-demo')], '')
    ctx.plant(['docker', ['exec', 'talaria-redis-demo', 'redis-cli', 'ping']], '')
    return ctx
  }

  test('one nothing names any more is removed — after the up that moved its containers off it', async () => {
    const ctx = worktreeCtx()
    ctx.plant(['docker', inspect], 'f00d\n')
    // every container on the host: the bridge, the shared fleet network, main's
    ctx.plant(['docker', ps], 'bridge\nbridge\ntalaria\ntalaria-dev_default,talaria\ntalaria-wt-demo_default2\n')
    await runDev(ctx)
    const up = ctx.calls.findIndex((c) => c.args.includes('up'))
    const removed = ctx.calls.findIndex((c) => c.cmd === 'docker' && same(c.args, rm))
    expect(up).toBeGreaterThanOrEqual(0)
    expect(removed).toBeGreaterThan(up)
    expect(ctx.logLines.some((l) => l.kind === 'ok' && l.msg.includes(NET))).toBe(true)
    expect(reachedApp(ctx)).toBe(true)
  })

  test('no such network — every worktree cut since — asks nothing more', async () => {
    const ctx = worktreeCtx()
    ctx.plant(['docker', inspect], new Error(`Error response from daemon: network ${NET} not found`))
    await runDev(ctx)
    expect(called(ctx, ps)).toBe(false)
    expect(called(ctx, rm)).toBe(false)
    expect(reachedApp(ctx)).toBe(true)
  })

  test('one a container still names is kept — a stopped one too, which could not start without it', async () => {
    for (const listing of [`bridge\n${NET}\n`, `talaria,${NET}\n`]) {
      const ctx = worktreeCtx()
      ctx.plant(['docker', inspect], 'f00d\n')
      ctx.plant(['docker', ps], listing)
      await runDev(ctx)
      expect(called(ctx, rm)).toBe(false)
      expect(reachedApp(ctx)).toBe(true)
    }
  })

  test('a refused or timed-out removal never stops the boot', async () => {
    for (const fail of [ps, rm]) {
      const ctx = worktreeCtx()
      ctx.plant(['docker', inspect], 'f00d\n')
      ctx.plant(['docker', fail], new Error('has active endpoints / timed out'))
      await runDev(ctx)
      expect(ctx.logLines.some((l) => l.kind === 'ok' && l.msg.includes(NET))).toBe(false)
      expect(reachedApp(ctx)).toBe(true)
    }
  })

  test('the main checkout touches no network', async () => {
    const root = makeTree()
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(ctx.calls.some((c) => c.cmd === 'docker' && c.args[0] === 'network')).toBe(false)
  })
})

// ── The app's port is strict, and a taken one names its holder (R23) ───────
describe('talaria dev — a taken app port', () => {
  const SS = (port: string) => ['-Hltnp', 'sport', '=', `:${port}`]
  const wtTree = () => {
    const root = makeTree({ uiEnv: 'TALARIA_WORKTREE=wt\nDATABASE_URL=x\nREDIS_URL=y\nPORT=5310\n' })
    writeFileSync(join(root, '.git'), 'gitdir: /elsewhere\n')
    return root
  }

  test('a listener with a visible pid stops dev before any infra, naming the pid and its command', async () => {
    const ctx = fakeCtx()
    ctx.root = wtTree()
    plantInfra(ctx)
    ctx.plant(['ss', SS('5310')], 'LISTEN 0      511          0.0.0.0:5310       0.0.0.0:*    users:(("node",pid=4242,fd=23))\n')
    ctx.plant(['ps', ['-o', 'args=', '-p', '4242']], 'node /work/ui/node_modules/.bin/vite dev --port 5310\n')
    const msg = await attempt(ctx)
    expect(msg).toContain("port 5310 (this worktree's PORT in ui/.env) is already taken by pid 4242 (node /work/ui/node_modules/.bin/vite dev --port 5310)")
    expect(ctx.calls.some((c) => c.args.includes('up'))).toBe(false)
    expect(ctx.calls.some((c) => c.cmd === 'bun' && c.args[1] === 'dev')).toBe(false)
  })

  test("the primary's 5273 is checked the same way", async () => {
    const root = makeTree({ uiEnv: 'DATABASE_URL=x\nREDIS_URL=y\n' })
    const ctx = fakeCtx()
    ctx.root = root
    plantInfra(ctx)
    ctx.plant(['ss', SS('5273')], 'LISTEN 0 511 *:5273 *:* users:(("bun",pid=77,fd=9))\n')
    const msg = await attempt(ctx)
    expect(msg).toContain("port 5273 (the dev script's port) is already taken by pid 77")
  })

  test('a port docker publishes names the container', async () => {
    const ctx = fakeCtx()
    ctx.plant(['ss', SS('5310')], 'LISTEN 0 4096 127.0.0.1:5310 0.0.0.0:*\n')
    ctx.plant(['docker', ['ps', '--filter', 'publish=5310', '--format', '{{.Names}}']], 'talaria-pg-other\n')
    expect(await portHolder(ctx, '5310')).toBe('container talaria-pg-other (published by docker)')
  })

  test("a holder this user cannot see is still named as taken", async () => {
    const ctx = fakeCtx()
    ctx.plant(['ss', SS('5310')], 'LISTEN 0 4096 127.0.0.1:5310 0.0.0.0:*\n')
    expect(await portHolder(ctx, '5310')).toContain('a process this user cannot see')
  })

  test('no ss: lsof answers (macOS)', async () => {
    const ctx = fakeCtx()
    ctx.plant(['ss', SS('5310')], new Error('spawn ss ENOENT'))
    ctx.plant(['lsof', ['-nP', '-iTCP:5310', '-sTCP:LISTEN', '-Fpc']], 'p912\ncnode\nf23\n')
    ctx.plant(['ps', ['-o', 'args=', '-p', '912']], new Error('gone'))
    expect(await portHolder(ctx, '5310')).toBe('pid 912 (node)')
  })

  test('nothing listening, or nothing that can say: null — vite --strictPort is the backstop', async () => {
    const free = fakeCtx()
    expect(await portHolder(free, '5310')).toBeNull()
    const blind = fakeCtx()
    blind.plant(['ss', SS('5310')], new Error('spawn ss ENOENT'))
    blind.plant(['lsof', ['-nP', '-iTCP:5310', '-sTCP:LISTEN', '-Fpc']], new Error('exit 1'))
    expect(await portHolder(blind, '5310')).toBeNull()
  })

  test('vite is always strict', () => {
    expect(viteArgs('5310')).toEqual(['run', 'dev', '--', '--port', '5310', '--strictPort'])
    expect(viteArgs(undefined)).toEqual(['run', 'dev', '--', '--strictPort'])
  })
})
