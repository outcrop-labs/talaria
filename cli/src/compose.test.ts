// compose.ts's readiness probe and its failure diagnosis — the two places the
// worktree and dev commands used to trust docker's first answer.

import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import {
  POOL_EXHAUSTED_RE,
  POOL_PROBE_NETWORK,
  WORKTREE_COMPOSE,
  compose,
  explainPoolExhaustion,
  pgReadyArgs,
  waitForPostgres,
  worktreeComposeFiles,
} from './compose'
import { fakeCtx } from './testing'

const ROOT = join(import.meta.dir, '..', '..')

describe('pgReadyArgs — a real query, over TCP', () => {
  test('TCP to 127.0.0.1 and a query against the talaria database, never the socket or pg_isready', () => {
    // The entrypoint's temporary init server listens on the socket only and
    // exists before the database does; `pg_isready -d` says "accepting
    // connections" for a database that does not exist. Both let a seed run
    // into "the database system is shutting down" (follow-up A1).
    const args = pgReadyArgs('talaria-pg-demo')
    expect(args).toEqual(['exec', 'talaria-pg-demo', 'psql', '-h', '127.0.0.1', '-U', 'talaria', '-d', 'talaria', '-tAc', 'select 1'])
    expect(args).not.toContain('pg_isready')
  })

  test('waitForPostgres answers false when the probe never passes, and true once it does', async () => {
    const down = fakeCtx()
    down.plant(['docker', pgReadyArgs('talaria-pg-demo')], new Error('connection refused'))
    expect(await waitForPostgres(down, 'talaria-pg-demo', 3)).toBe(false)
    expect(down.calls.filter((c) => c.args.includes('select 1'))).toHaveLength(3)

    const up = fakeCtx()
    expect(await waitForPostgres(up, 'talaria-pg-demo', 3)).toBe(true)
    expect(up.calls).toHaveLength(1)
  })
})

describe('the worktree stack file', () => {
  test('worktreeComposeFiles: sidecars, dev, then the worktree file — last, so it wins', () => {
    expect(worktreeComposeFiles('/r')).toEqual([
      '/r/docker/sidecars.compose.yml',
      '/r/docker/dev-compose.yml',
      join('/r', WORKTREE_COMPOSE),
    ])
  })

  test('it puts exactly the two services a worktree runs on the default bridge, and decides nothing else', () => {
    // No project network means no subnet from the daemon's ~30 address
    // pools (follow-up A8). Any other key here would be a third definition of
    // a sidecar the fragment owns.
    const body = readFileSync(join(ROOT, WORKTREE_COMPOSE), 'utf8')
      .split('\n')
      .filter((l) => l.trim() && !l.trimStart().startsWith('#'))
    expect(body).toEqual(['services:', '  postgres:', '    network_mode: bridge', '  redis:', '    network_mode: bridge'])
  })

  test("the shared postgres healthcheck probes over TCP, as the CLI's wait does", () => {
    const sidecars = readFileSync(join(ROOT, 'docker/sidecars.compose.yml'), 'utf8')
    expect(sidecars).toContain("test: ['CMD-SHELL', 'pg_isready -h 127.0.0.1 -U talaria -d talaria']")
    expect(sidecars).not.toContain("'pg_isready -U talaria -d talaria'")
  })
})

describe('a failed up — is docker out of network address pools?', () => {
  const POOL_ERROR =
    'Command failed: docker network create --label talaria.pool-probe=1 talaria-pool-probe-1\n' +
    'Error response from daemon: all predefined address pools have been fully subnetted'
  const create = ['network', 'create', '--label', 'talaria.pool-probe=1', POOL_PROBE_NETWORK]

  test("docker's pool messages, old and new, are recognised", () => {
    expect(POOL_EXHAUSTED_RE.test('failed to create network talaria-wt-fu-crons_default: Error response from daemon: all predefined address pools have been fully subnetted')).toBe(true)
    expect(POOL_EXHAUSTED_RE.test('could not find an available, non-overlapping IPv4 address pool among the defaults to assign to the network')).toBe(true)
    expect(POOL_EXHAUSTED_RE.test('port is already allocated')).toBe(false)
  })

  test('pool exhausted: says what it means and points at cleanup', async () => {
    const ctx = fakeCtx()
    ctx.plant(['docker', create], new Error(POOL_ERROR))
    expect(await explainPoolExhaustion(ctx)).toBe(true)
    const warns = ctx.logLines.filter((l) => l.kind === 'warn').map((l) => l.msg)
    expect(warns).toHaveLength(1)
    expect(warns[0]).toContain('address pool is fully subnetted')
    expect(warns[0]).toContain('bun talaria cleanup --apply')
    // the probe was never made, so there is nothing to remove
    expect(ctx.calls.some((c) => c.args[1] === 'rm')).toBe(false)
  })

  test('room to spare: the probe network is removed again and nothing is said', async () => {
    const ctx = fakeCtx()
    expect(await explainPoolExhaustion(ctx)).toBe(false)
    expect(ctx.calls.map((c) => c.args)).toEqual([create, ['network', 'rm', POOL_PROBE_NETWORK]])
    expect(ctx.logLines).toEqual([])
  })

  test('some other create failure is not ours to explain', async () => {
    const ctx = fakeCtx()
    ctx.plant(['docker', create], new Error('Cannot connect to the Docker daemon'))
    expect(await explainPoolExhaustion(ctx)).toBe(false)
    expect(ctx.logLines).toEqual([])
  })

  test('compose asks only after a failed up — never after a good one, never after down', async () => {
    const spec = { files: ['/r/a.yml'], project: 'talaria-wt-demo' }
    const failing = fakeCtx()
    failing.plant(['docker', ['compose', '-f', '/r/a.yml', '-p', 'talaria-wt-demo', 'up', '-d', 'postgres']], new Error('exit 1'))
    failing.plant(['docker', create], new Error(POOL_ERROR))
    expect(await compose(failing, spec, ['up', '-d', 'postgres'])).toBe(1)
    expect(failing.logLines.some((l) => l.kind === 'warn' && l.msg.includes('talaria cleanup'))).toBe(true)

    const fine = fakeCtx()
    expect(await compose(fine, spec, ['up', '-d', 'postgres'])).toBe(0)
    expect(fine.calls).toHaveLength(1)

    const down = fakeCtx()
    down.plant(['docker', ['compose', '-f', '/r/a.yml', '-p', 'talaria-wt-demo', 'down']], new Error('exit 1'))
    expect(await compose(down, spec, ['down'])).toBe(1)
    expect(down.calls).toHaveLength(1)
  })
})
