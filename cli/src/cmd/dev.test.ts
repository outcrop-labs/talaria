// dev.sh's decision table, as tests: the mcp staleness probe, the skip/devbox
// gates, the worktree guard, and the S3 lift. Filesystem state is real (tmp
// trees); process behavior is planted. `die` throws CliError (the dispatcher
// is what prints it), so refusals are asserted on the throw.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'
import { runDev, rustApi } from './dev'
import { fakeCtx, type FakeCtx } from '../testing'
import { CliError } from '../ui'

const makeTree = (over: { distMtime?: Date; uiEnv?: string } = {}) => {
  const root = mkdtempSync(join(tmpdir(), 'talaria-dev-'))
  mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
  mkdirSync(join(root, 'mcp/node_modules'), { recursive: true })
  mkdirSync(join(root, 'mcp/src/deep'), { recursive: true })
  mkdirSync(join(root, 'mcp/dist'), { recursive: true })
  writeFileSync(join(root, 'mcp/src/index.ts'), 'x')
  writeFileSync(join(root, 'mcp/src/deep/util.ts'), 'x')
  writeFileSync(join(root, 'mcp/dist/index.js'), 'built')
  mkdirSync(join(root, 'docker/searxng'), { recursive: true })
  writeFileSync(join(root, 'docker/searxng/settings.template.yml'), 'secret_key: "__SEARXNG_SECRET__"\n')
  writeFileSync(join(root, 'ui/.env'), over.uiEnv ?? 'DATABASE_URL=x\nREDIS_URL=y\n')
  const distTime = over.distMtime ?? new Date()
  utimesSync(join(root, 'mcp/dist/index.js'), distTime, distTime)
  utimesSync(join(root, 'mcp/src/index.ts'), distTime, distTime)
  return root
}

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
    expect(ctx.calls.some((c) => c.cmd === 'bun' && c.args[1] === 'build')).toBe(false)
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

  test('TALARIA_SKIP_MCP_BUILD=1 → never builds', async () => {
    const root = makeTree({ distMtime: new Date(0) })
    const ctx = fakeCtx({ env: { TALARIA_SKIP_MCP_BUILD: '1' } })
    ctx.root = root
    plantInfra(ctx)
    await runDev(ctx)
    expect(ctx.calls.some((c) => c.args[1] === 'build')).toBe(false)
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

  test('worktree mode aims compose at its own project and probes its own containers', async () => {
    const root = makeTree({
      uiEnv:
        'TALARIA_WORKTREE=demo\nDATABASE_URL=postgres://talaria:talaria@127.0.0.1:5601/talaria\nREDIS_URL=redis://127.0.0.1:6501\nPORT=5301\n',
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
    const root = makeTree({ uiEnv: 'TALARIA_WORKTREE=wt\nDATABASE_URL=x\nREDIS_URL=y\nPORT=5310\n' })
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

  test('off by default — TALARIA_API unset touches nothing', async () => {
    const ctx = fakeCtx()
    await rustApi(ctx, 'DATABASE_URL=x\n')
    expect(ctx.calls).toHaveLength(0)
    expect(ctx.logLines).toHaveLength(0)
  })

  test('no cargo on PATH → warn and skip (the TS server serves everything)', async () => {
    const port = await closedPort()
    const ctx = fakeCtx({ env: { TALARIA_API: 'on', TALARIA_API_PORT: String(port) } })
    ctx.plant(['cargo', ['--version']], new Error('not found'))
    await rustApi(ctx, 'DATABASE_URL=x\n')
    // the proxy-URL warning fires in every on-path — ui/.env here has none
    expect(ctx.logLines.filter((l) => l.kind === 'warn').map((l) => l.msg)).toEqual([
      expect.stringContaining('TALARIA_RUST_API_URL is not set'),
      'TALARIA_API=on but no cargo on PATH — skipping the Rust api (the TS server serves everything).',
    ])
    // looked for cargo, but never tried to run it
    expect(ctx.calls.map((c) => [c.cmd, ...c.args])).toEqual([['cargo', '--version']])
  })

  test('something already answering on the port → adopt, never check cargo', async () => {
    await withListener(async (port) => {
      const ctx = fakeCtx({ env: { TALARIA_API: 'on', TALARIA_API_PORT: String(port) } })
      await rustApi(ctx, `TALARIA_RUST_API_URL=http://127.0.0.1:${port}\n`)
      expect(ctx.logLines).toHaveLength(1)
      expect(ctx.logLines[0]).toMatchObject({ kind: 'say', msg: `rust api → already listening on :${port}; adopting it` })
      expect(ctx.calls).toHaveLength(0)
    })
  })
})
