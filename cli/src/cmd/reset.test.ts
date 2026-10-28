// reset.sh's safety rails, as tests: the localhost-only guard (BEFORE
// anything), the typed confirm (wrong word = zero destructive execs), the
// fleet teardown scope, and WHICH STACK every target is: inside a worktree,
// the worktree's own postgres and fleet project, never main's. `die` throws
// CliError, so refusals assert on the throw; the counts/warnings print
// through the planted log.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { isLocalDb, runReset } from './reset'
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

const makeTree = (dbUrl: string, extra = '', opts: { linked?: boolean } = {}): string => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'talaria-reset-')))
  mkdirSync(join(root, 'ui'), { recursive: true })
  if (opts.linked) writeFileSync(join(root, '.git'), 'gitdir: /x/.git/worktrees/demo\n')
  writeFileSync(join(root, 'ui/.env'), `DATABASE_URL=${dbUrl}\n${extra}`)
  return root
}

/** A worktree's ui/.env, as `talaria worktree demo` writes it. */
const WT = 'TALARIA_WORKTREE=demo\nTALARIA_FLEET_PROJECT=talaria-wt-demo-fleet\n'
const WT_DB = 'postgres://talaria:talaria@127.0.0.1:5605/talaria'

const inspectJson = (project: string, port: string): string =>
  JSON.stringify([
    {
      State: { Running: true },
      Config: { Labels: { 'com.docker.compose.project': project } },
      NetworkSettings: { Ports: { '5432/tcp': [{ HostIp: '127.0.0.1', HostPort: port }] } },
    },
  ])

/** The primary's postgres, up, on the port the fixture's DATABASE_URL names. */
const plantUp = (ctx: FakeCtx): void => {
  ctx.plant(['docker', ['inspect', 'talaria-postgres-dev']], inspectJson('talaria-dev', '5544'))
}
/** The worktree's own postgres, up, in its own project on its own port. */
const plantWorktreeUp = (ctx: FakeCtx): void => {
  ctx.plant(['docker', ['inspect', 'talaria-pg-demo']], inspectJson('talaria-wt-demo', '5605'))
}
const psByProject = (project: string, all: boolean): [string, string[]] => [
  'docker',
  ['ps', ...(all ? ['-a'] : []), '--filter', `label=com.docker.compose.project=${project}`, '--format', '{{.Names}}'],
]

/** Every docker call the run made that names a container or project of
 *  MAIN's stack. */
const touchesMain = (ctx: FakeCtx) =>
  ctx.calls.filter((c) =>
    c.args.some((a) => a === 'talaria-postgres-dev' || a === 'talaria-redis-dev' || /^talaria-fleet(-|$)/.test(a) || a.endsWith('=talaria-fleet')),
  )

const destructive = (ctx: FakeCtx) =>
  ctx.calls.filter((c) => c.args.includes('ON_ERROR_STOP=1') || (c.args[0] === 'rm' && c.args[1] === '-f') || c.args.includes('down'))

describe('reset — localhost guard', () => {
  test('isLocalDb accepts loopback spellings only', () => {
    expect(isLocalDb('postgres://talaria:talaria@127.0.0.1:5544/talaria')).toBe(true)
    expect(isLocalDb('postgres://talaria:talaria@localhost:5432/talaria')).toBe(true)
    expect(isLocalDb('postgres://talaria:talaria@prod.example.com:5432/talaria')).toBe(false)
    expect(isLocalDb('postgres://talaria:talaria@10.0.0.4:5432/talaria')).toBe(false)
  })

  test('a remote DATABASE_URL dies before a single exec runs', async () => {
    const ctx = fakeCtx()
    ctx.root = makeTree('postgres://u:p@prod.example.com:5432/talaria')
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toContain('does not point at localhost')
    expect(ctx.calls.length).toBe(0)
  })

  test('missing ui/.env dies pointing at setup', async () => {
    const ctx = fakeCtx()
    ctx.root = mkdtempSync(join(tmpdir(), 'talaria-empty-'))
    const msg = await attempt(() => runReset(ctx, 'secrets'))
    expect(msg).toContain('bun talaria setup')
  })

  test('postgres down dies before the counts', async () => {
    const ctx = fakeCtx()
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    const msg = await attempt(() => runReset(ctx, 'secrets'))
    expect(msg).toContain('is not running')
    expect(ctx.calls.filter((c) => c.args.includes('psql'))).toHaveLength(0)
  })
})

describe('reset — typed confirm', () => {
  test('wrong word aborts with zero destructive execs recorded', async () => {
    const ctx = fakeCtx({ reply: 'secrets ' }) // trailing space: trimmed by a shell's `read`, and by us
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    plantUp(ctx)
    // counts all plant to 0 by default (unplanted exec → empty stdout)
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toBe('aborted — nothing was changed')
    expect(destructive(ctx)).toHaveLength(0)
  })

  test('the right word (after trim) proceeds to the batch', async () => {
    const ctx = fakeCtx({ reply: 'database' })
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    plantUp(ctx)
    await runReset(ctx, 'database')
    const batch = ctx.calls.find((c) => c.args.includes('ON_ERROR_STOP=1'))!
    expect(batch.args.join(' ')).toContain('drop database if exists talaria')
    expect(batch.args.join(' ')).toContain('-d postgres')
    expect(batch.args).toContain('talaria-postgres-dev')
  })

  test('the drop is a -c of its own: DROP DATABASE refuses to run inside a transaction block', async () => {
    // One `-c` with all three statements is one transaction, and Postgres
    // refused the drop in it every time (found running this live).
    const ctx = fakeCtx({ reply: 'database' })
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    plantUp(ctx)
    await runReset(ctx, 'database')
    const batch = ctx.calls.find((c) => c.args.includes('ON_ERROR_STOP=1'))!
    const statements = batch.args.flatMap((a, i) => (batch.args[i - 1] === '-c' ? [a] : []))
    expect(statements).toHaveLength(3)
    expect(statements[0]).toContain('pg_terminate_backend')
    expect(statements[1]).toBe('drop database if exists talaria with (force)')
    expect(statements[2]).toBe('create database talaria owner talaria')
    for (const st of statements) expect(st).not.toContain(';')
  })
})

describe('reset — secrets scope', () => {
  test('clears ciphertext first, keys last, in one batch', async () => {
    const ctx = fakeCtx({ reply: 'secrets' })
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    plantUp(ctx)
    await runReset(ctx, 'secrets')
    const batch = ctx.calls.find((c) => c.args.includes('ON_ERROR_STOP=1'))!
    const sql = batch.args[batch.args.indexOf('-c') + 1]!
    expect(sql).toContain('update llm_endpoints set api_key_cipher = null')
    expect(sql.indexOf('update llm_endpoints')).toBeLessThan(sql.indexOf('delete from secret_keys'))
    expect(sql.startsWith('begin;')).toBe(true)
    expect(sql.endsWith('commit;')).toBe(true)
  })

  test('every count prints on its own line', async () => {
    const ctx = fakeCtx({ reply: 'no' })
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    plantUp(ctx)
    await attempt(() => runReset(ctx, 'secrets'))
    const rows = ctx.logLines.filter((l) => l.kind === 'raw' && l.msg.trim() !== '')
    expect(rows.length).toBeGreaterThan(5)
    for (const r of rows) expect(r.msg.endsWith('\n')).toBe(true)
  })
})

describe('reset — fleet scope', () => {
  test('removes rendered artifacts, keeps fleet/chassis.yml + skills', async () => {
    const root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    mkdirSync(join(root, 'fleet/agents/x'), { recursive: true })
    writeFileSync(join(root, 'fleet/chassis.yml'), 'network:\n  name: talaria\n')
    writeFileSync(join(root, 'fleet/.env'), 'LLM_API_KEY=x\n')
    const ctx = fakeCtx({ reply: 'fleet' })
    ctx.root = root
    plantUp(ctx)
    await runReset(ctx, 'fleet')
    expect(ctx.logLines.some((l) => l.msg.includes('agent_defs in the database are KEPT'))).toBe(true)
    // rendered artifacts gone; user-owned config plane intact
    expect(await Bun.file(join(root, 'fleet/.env')).exists()).toBe(false)
    expect(await Bun.file(join(root, 'fleet/agents/x')).exists()).toBe(false)
    expect(await Bun.file(join(root, 'fleet/chassis.yml')).exists()).toBe(true)
  })

  test("leftover containers of the primary's fleet project are force-removed, by label", async () => {
    const ctx = fakeCtx({ reply: 'fleet' })
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    plantUp(ctx)
    ctx.plant(psByProject('talaria-fleet', true), 'talaria-fleet-agent-a-1\ntalaria-fleet-agent-b-1\n')
    await runReset(ctx, 'fleet')
    expect(ctx.calls.some((c) => c.args[0] === 'rm' && c.args.includes('talaria-fleet-agent-a-1'))).toBe(true)
    expect(ctx.calls.some((c) => c.args[0] === 'rm' && c.args.includes('talaria-fleet-agent-b-1'))).toBe(true)
    expect(ctx.calls.some((c) => c.args[0] === 'rm' && c.args.includes('talaria-postgres-dev'))).toBe(false)
    // never the old unfiltered listing a name prefix was applied to
    expect(ctx.calls.some((c) => c.args[0] === 'ps' && !c.args.includes('--filter'))).toBe(false)
  })

  test('the compose down names the project, so a rendered file cannot pick another', async () => {
    const root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    mkdirSync(join(root, 'fleet'), { recursive: true })
    writeFileSync(join(root, 'fleet/docker-compose.yml'), 'name: talaria-fleet\nservices: {}\n')
    writeFileSync(join(root, 'fleet/.env'), 'LLM_API_KEY=x\n')
    const ctx = fakeCtx({ reply: 'fleet' })
    ctx.root = root
    plantUp(ctx)
    await runReset(ctx, 'fleet')
    const down = ctx.calls.find((c) => c.args.includes('down'))!
    expect(down.args.slice(down.args.indexOf('-p'), down.args.indexOf('-p') + 2)).toEqual(['-p', 'talaria-fleet'])
    expect(down.args).toContain('--env-file')
  })
})

describe('reset — inside a worktree, every target is the worktree\'s own', () => {
  for (const mode of ['secrets', 'database'] as const) {
    test(`${mode}: the counts and the batch run in talaria-pg-demo, nothing names main`, async () => {
      const ctx = fakeCtx({ reply: mode })
      ctx.root = makeTree(WT_DB, WT, { linked: true })
      plantWorktreeUp(ctx)
      await runReset(ctx, mode)
      const execs = ctx.calls.filter((c) => c.args[0] === 'exec')
      expect(execs.length).toBeGreaterThan(1)
      for (const c of execs) expect(c.args[1]).toBe('talaria-pg-demo')
      expect(ctx.calls.some((c) => c.args.includes('ON_ERROR_STOP=1'))).toBe(true)
      expect(touchesMain(ctx)).toHaveLength(0)
    })
  }

  test('fleet: the down and the leftovers are the worktree fleet project, never talaria-fleet-agent*', async () => {
    const root = makeTree(WT_DB, WT, { linked: true })
    mkdirSync(join(root, 'fleet/agents/eng'), { recursive: true })
    writeFileSync(join(root, 'fleet/docker-compose.yml'), '# Generated\nname: talaria-wt-demo-fleet\nservices: {}\n')
    const ctx = fakeCtx({ reply: 'fleet' })
    ctx.root = root
    plantWorktreeUp(ctx)
    ctx.plant(psByProject('talaria-wt-demo-fleet', true), 'talaria-wt-demo-fleet-agent-eng-1\n')
    // What the old name-prefix sweep would have found: main's agents.
    ctx.plant(['docker', ['ps', '-a', '--format', '{{.Names}}']], 'talaria-fleet-agent-eng-1\ntalaria-wt-demo-fleet-agent-eng-1\n')
    await runReset(ctx, 'fleet')
    const down = ctx.calls.find((c) => c.args.includes('down'))!
    expect(down.args).toContain('talaria-wt-demo-fleet')
    const removed = ctx.calls.filter((c) => c.args[0] === 'rm').map((c) => c.args[2])
    expect(removed).toEqual(['talaria-wt-demo-fleet-agent-eng-1'])
    expect(touchesMain(ctx)).toHaveLength(0)
    expect(await Bun.file(join(root, 'fleet/docker-compose.yml')).exists()).toBe(false)
  })

  test('the plan names only the worktree\'s objects, and an abort changes nothing', async () => {
    const ctx = fakeCtx({ reply: 'no' })
    ctx.root = makeTree(WT_DB, WT, { linked: true })
    plantWorktreeUp(ctx)
    ctx.plant(psByProject('talaria-wt-demo-fleet', true), 'talaria-wt-demo-fleet-agent-eng-1\n')
    const msg = await attempt(() => runReset(ctx, 'fleet'))
    expect(msg).toBe('aborted — nothing was changed')
    const plan = ctx.logLines.map((l) => l.msg).join('')
    expect(plan).toContain('Target — worktree "demo"')
    expect(plan).toContain('talaria-pg-demo (compose project talaria-wt-demo, 127.0.0.1:5605)')
    expect(plan).toContain('talaria-wt-demo-fleet')
    expect(plan).toContain('talaria-wt-demo-fleet-agent-eng-1')
    expect(plan).not.toContain('talaria-postgres-dev')
    expect(destructive(ctx)).toHaveLength(0)
  })

  test('a shell TALARIA_PG_CONTAINER naming main dies before any docker call', async () => {
    const ctx = fakeCtx({ reply: 'database', env: { TALARIA_PG_CONTAINER: 'talaria-postgres-dev' } })
    ctx.root = makeTree(WT_DB, WT, { linked: true })
    plantWorktreeUp(ctx)
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toContain('TALARIA_PG_CONTAINER=talaria-postgres-dev')
    expect(ctx.calls).toHaveLength(0)
  })

  test("a postgres in another compose project is not this worktree's: dies before the counts", async () => {
    const ctx = fakeCtx({ reply: 'database' })
    ctx.root = makeTree(WT_DB, WT, { linked: true })
    ctx.plant(['docker', ['inspect', 'talaria-pg-demo']], inspectJson('talaria-wt-other', '5605'))
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toContain("compose project 'talaria-wt-other'")
    expect(ctx.calls.filter((c) => c.args.includes('psql'))).toHaveLength(0)
  })

  test("a postgres on another port than DATABASE_URL's dies before the counts", async () => {
    const ctx = fakeCtx({ reply: 'database' })
    ctx.root = makeTree(WT_DB, WT, { linked: true })
    ctx.plant(['docker', ['inspect', 'talaria-pg-demo']], inspectJson('talaria-wt-demo', '5544'))
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toContain('DATABASE_URL names port 5605')
    expect(ctx.calls.filter((c) => c.args.includes('psql'))).toHaveLength(0)
  })

  test('fleet with no TALARIA_FLEET_PROJECT dies: its agents may be in main\'s project', async () => {
    const ctx = fakeCtx({ reply: 'fleet' })
    ctx.root = makeTree(WT_DB, 'TALARIA_WORKTREE=demo\n', { linked: true })
    plantWorktreeUp(ctx)
    const msg = await attempt(() => runReset(ctx, 'fleet'))
    expect(msg).toContain('names no TALARIA_FLEET_PROJECT')
    expect(destructive(ctx)).toHaveLength(0)
    expect(ctx.calls.some((c) => c.args[0] === 'ps')).toBe(false)
  })

  test('a rendered fleet file naming another project dies before the confirm', async () => {
    const root = makeTree(WT_DB, WT, { linked: true })
    mkdirSync(join(root, 'fleet'), { recursive: true })
    writeFileSync(join(root, 'fleet/docker-compose.yml'), 'name: talaria-fleet\nservices: {}\n')
    const ctx = fakeCtx({ reply: 'fleet' })
    ctx.root = root
    plantWorktreeUp(ctx)
    const msg = await attempt(() => runReset(ctx, 'fleet'))
    expect(msg).toContain("names compose project 'talaria-fleet'")
    expect(destructive(ctx)).toHaveLength(0)
    expect(await Bun.file(join(root, 'fleet/docker-compose.yml')).exists()).toBe(true)
  })

  test('a linked git worktree without the marker dies: its stack is a guess', async () => {
    const ctx = fakeCtx({ reply: 'database' })
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria', '', { linked: true })
    plantUp(ctx)
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toContain('git worktree without an isolated stack')
    expect(ctx.calls).toHaveLength(0)
  })
})

describe('reset — the database and role it execs as', () => {
  for (const [what, url] of [
    ['another database', 'postgres://talaria:talaria@127.0.0.1:5544/other'],
    ['another role', 'postgres://bob:pw@127.0.0.1:5544/talaria'],
  ] as const) {
    test(`a DATABASE_URL naming ${what} dies before any docker call: reset runs psql -U talaria -d talaria`, async () => {
      const ctx = fakeCtx({ reply: 'database' })
      ctx.root = makeTree(url)
      plantUp(ctx)
      const msg = await attempt(() => runReset(ctx, 'database'))
      expect(msg).toContain('not the database DATABASE_URL names')
      expect(ctx.calls).toHaveLength(0)
    })
  }
})

describe("reset fleet — the primary never acts on a worktree's fleet", () => {
  test("a shell TALARIA_FLEET_PROJECT naming a worktree's project dies before the listing", async () => {
    const ctx = fakeCtx({ reply: 'fleet', env: { TALARIA_FLEET_PROJECT: 'talaria-wt-other-fleet' } })
    ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    plantUp(ctx)
    ctx.plant(psByProject('talaria-wt-other-fleet', true), 'talaria-wt-other-fleet-agent-a-1\n')
    const msg = await attempt(() => runReset(ctx, 'fleet'))
    expect(msg).toContain("TALARIA_FLEET_PROJECT=talaria-wt-other-fleet is a worktree's fleet project")
    expect(ctx.calls.some((c) => c.args[0] === 'ps')).toBe(false)
    expect(destructive(ctx)).toHaveLength(0)
  })

  test("a shell under a worktree's `talaria dev` (its TALARIA_PG_CONTAINER inherited) dies in the primary before any docker call", async () => {
    for (const mode of ['secrets', 'database', 'fleet'] as const) {
      const ctx = fakeCtx({ reply: mode, env: { TALARIA_PG_CONTAINER: 'talaria-pg-other', TALARIA_PG_PORT: '5605' } })
      ctx.root = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
      plantUp(ctx)
      const msg = await attempt(() => runReset(ctx, mode))
      expect(msg).toContain('TALARIA_PG_CONTAINER=talaria-pg-other   (worktree "other"\'s postgres)')
      expect(msg).toContain(`env -u TALARIA_PG_CONTAINER -u TALARIA_PG_PORT bun talaria reset ${mode}`)
      expect(ctx.calls).toHaveLength(0)
    }
  })
})

describe('reset — the shim run from another checkout', () => {
  test("standing in a worktree with the primary's CLI dies before anything", async () => {
    // ctx.root is the CLI's own checkout (the shim's: the primary); the
    // operator stands in a worktree.
    const primary = makeTree('postgres://talaria:talaria@127.0.0.1:5544/talaria')
    mkdirSync(join(primary, '.git'))
    mkdirSync(join(primary, 'cli/bin'), { recursive: true })
    writeFileSync(join(primary, 'cli/bin/talaria.ts'), '')
    const wt = makeTree(WT_DB, WT, { linked: true })
    mkdirSync(join(wt, 'cli/bin'), { recursive: true })
    writeFileSync(join(wt, 'cli/bin/talaria.ts'), '')
    const ctx = fakeCtx({ reply: 'database', cwd: wt })
    ctx.root = primary
    plantUp(ctx)
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toContain(`standing in ${wt}`)
    expect(msg).toContain(`cd ${wt} && bun talaria reset database`)
    expect(ctx.calls).toHaveLength(0)
  })
})
