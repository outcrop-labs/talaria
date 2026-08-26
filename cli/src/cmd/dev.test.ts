// dev.sh's decision table, as tests: the mcp staleness probe, the skip/devbox
// gates, the worktree guard, and the S3 lift. Filesystem state is real (tmp
// trees); process behavior is planted. `die` throws CliError (the dispatcher
// is what prints it), so refusals are asserted on the throw.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { runDev } from './dev'
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
