// stack.ts as tests: which stack a checkout owns, in a worktree and in the
// primary, and every refusal the module header lists. `die` throws CliError,
// so refusals assert on the throw.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, realpathSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  ALL_PRIMARY_CONTAINER_VARS, checkoutAt, composeFileProject, containerProject, devStack, inside, isSideProject, physical,
  primaryContainer, printable, projectContainers, refuseForeignCheckout, refuseSideStackContainers, refuseWorktreeEnvOverride,
  sideStackContainers, sideStackOfName, verifyPostgres, WORKTREE_INHERITED_VARS, WORKTREE_OWN_VARS, worktreeStackNames,
  type DevStack,
} from './stack'
import { WORKTREE_ENV_KEYS } from './cmd/worktree'
import { fakeCtx, type FakeCtx } from './testing'
import { CliError } from './ui'

const attempt = (fn: () => unknown): string => {
  try {
    fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}
const attemptAsync = async (fn: () => Promise<unknown>): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

/** A checkout-shaped tree: a .git (dir, or a file for a linked worktree),
 *  the CLI's entry point and a ui/.env. */
const checkout = (uiEnv: string, opts: { linked?: boolean } = {}): string => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'talaria-stack-')))
  if (opts.linked) writeFileSync(join(root, '.git'), 'gitdir: /somewhere/.git/worktrees/x\n')
  else mkdirSync(join(root, '.git'))
  mkdirSync(join(root, 'cli/bin'), { recursive: true })
  writeFileSync(join(root, 'cli/bin/talaria.ts'), '')
  mkdirSync(join(root, 'ui'), { recursive: true })
  writeFileSync(join(root, 'ui/.env'), uiEnv)
  return root
}

const WT_ENV =
  'DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5605/talaria\nTALARIA_WORKTREE=demo\nTALARIA_FLEET_PROJECT=talaria-wt-demo-fleet\n'

const inspectJson = (o: { running?: boolean; project?: string; port?: string }): string =>
  JSON.stringify([
    {
      State: { Running: o.running ?? true },
      Config: { Labels: o.project === undefined ? {} : { 'com.docker.compose.project': o.project } },
      NetworkSettings: { Ports: o.port === undefined ? {} : { '5432/tcp': [{ HostIp: '127.0.0.1', HostPort: o.port }] } },
    },
  ])

describe('worktreeStackNames', () => {
  test('the names `talaria worktree` creates (worktree.test.ts asserts the same literals)', () => {
    expect(worktreeStackNames('demo')).toEqual({
      pg: 'talaria-pg-demo',
      redis: 'talaria-redis-demo',
      infra: 'talaria-wt-demo',
      fleet: 'talaria-wt-demo-fleet',
    })
  })
})

describe('devStack — in a worktree', () => {
  test("every name is the worktree's own, none is main's", () => {
    const ctx = fakeCtx()
    ctx.root = checkout(WT_ENV, { linked: true })
    const s = devStack(ctx)
    expect(s).toEqual({
      kind: 'worktree',
      worktree: 'demo',
      pgContainer: 'talaria-pg-demo',
      redisContainer: 'talaria-redis-demo',
      infraProject: 'talaria-wt-demo',
      fleetProject: 'talaria-wt-demo-fleet',
      fleetProblem: null,
    })
    expect(JSON.stringify(s)).not.toContain('talaria-postgres-dev')
    expect(JSON.stringify(s)).not.toContain('talaria-dev')
  })

  test('a shell TALARIA_PG_CONTAINER naming main dies instead of winning', () => {
    const ctx = fakeCtx({ env: { TALARIA_PG_CONTAINER: 'talaria-postgres-dev' } })
    ctx.root = checkout(WT_ENV, { linked: true })
    const msg = attempt(() => devStack(ctx))
    expect(msg).toContain('TALARIA_PG_CONTAINER=talaria-postgres-dev')
    expect(msg).toContain('talaria-pg-demo')
  })

  test("a shell var that names the worktree's own container is not a conflict", () => {
    const ctx = fakeCtx({ env: { TALARIA_PG_CONTAINER: 'talaria-pg-demo', TALARIA_REDIS_CONTAINER: 'talaria-redis-demo' } })
    ctx.root = checkout(WT_ENV, { linked: true })
    expect(devStack(ctx).pgContainer).toBe('talaria-pg-demo')
  })

  test('a shell TALARIA_REDIS_CONTAINER naming another stack dies', () => {
    const ctx = fakeCtx({ env: { TALARIA_REDIS_CONTAINER: 'talaria-redis-dev' } })
    ctx.root = checkout(WT_ENV, { linked: true })
    expect(attempt(() => devStack(ctx))).toContain('TALARIA_REDIS_CONTAINER=talaria-redis-dev')
  })

  test('a shell TALARIA_WORKTREE naming another worktree dies', () => {
    const ctx = fakeCtx({ env: { TALARIA_WORKTREE: 'other' } })
    ctx.root = checkout(WT_ENV, { linked: true })
    expect(attempt(() => devStack(ctx))).toContain('TALARIA_WORKTREE=other')
  })

  test('no TALARIA_FLEET_PROJECT: the fleet cannot be named, and the reason says main', () => {
    const ctx = fakeCtx()
    ctx.root = checkout('DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5605/talaria\nTALARIA_WORKTREE=demo\n', { linked: true })
    const s = devStack(ctx)
    expect(s.pgContainer).toBe('talaria-pg-demo')
    expect(s.fleetProject).toBeNull()
    expect(s.fleetProblem).toContain("talaria-fleet, which is main's")
    expect(s.fleetProblem).toContain('TALARIA_FLEET_PROJECT=talaria-wt-demo-fleet')
  })

  test("a fleet project that is not the worktree's own is refused, by ui/.env or by the shell", () => {
    const fromFile = fakeCtx()
    fromFile.root = checkout(WT_ENV.replace('talaria-wt-demo-fleet', 'talaria-fleet'), { linked: true })
    expect(devStack(fromFile).fleetProject).toBeNull()
    const fromShell = fakeCtx({ env: { TALARIA_FLEET_PROJECT: 'talaria-wt-other-fleet' } })
    fromShell.root = checkout(WT_ENV, { linked: true })
    const s = devStack(fromShell)
    expect(s.fleetProject).toBeNull()
    expect(s.fleetProblem).toContain('talaria-wt-other-fleet')
  })
})

describe('devStack — in the primary', () => {
  test("the dev-compose defaults, the shell's override honoured", () => {
    const ctx = fakeCtx()
    ctx.root = checkout('DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\n')
    expect(devStack(ctx)).toEqual({
      kind: 'primary',
      worktree: null,
      pgContainer: 'talaria-postgres-dev',
      redisContainer: 'talaria-redis-dev',
      infraProject: 'talaria-dev',
      fleetProject: 'talaria-fleet',
      fleetProblem: null,
    })
    const over = fakeCtx({ env: { TALARIA_PG_CONTAINER: 'my-pg', TALARIA_FLEET_PROJECT: 'my-fleet' } })
    over.root = ctx.root
    expect(devStack(over).pgContainer).toBe('my-pg')
    expect(devStack(over).fleetProject).toBe('my-fleet')
  })

  test("ui/.env's TALARIA_FLEET_PROJECT is read, the shell wins over it", () => {
    const ctx = fakeCtx()
    ctx.root = checkout('DATABASE_URL=x\nTALARIA_FLEET_PROJECT=from-file\n')
    expect(devStack(ctx).fleetProject).toBe('from-file')
    const shell = fakeCtx({ env: { TALARIA_FLEET_PROJECT: 'from-shell' } })
    shell.root = ctx.root
    expect(devStack(shell).fleetProject).toBe('from-shell')
  })

  test("a worktree's fleet project is not the primary's: refused, from the shell or ui/.env", () => {
    const shell = fakeCtx({ env: { TALARIA_FLEET_PROJECT: 'talaria-wt-other-fleet' } })
    shell.root = checkout('DATABASE_URL=x\n')
    const s = devStack(shell)
    expect(s.fleetProject).toBeNull()
    expect(s.fleetProblem).toContain("talaria-wt-other-fleet is a worktree's fleet project")
    const file = fakeCtx()
    file.root = checkout('DATABASE_URL=x\nTALARIA_FLEET_PROJECT=talaria-wt-other-fleet\n')
    expect(devStack(file).fleetProject).toBeNull()
  })

  test("a shell container shaped like a worktree's or a box's is refused, and an empty one is the default", () => {
    for (const [key, value] of [
      ['TALARIA_PG_CONTAINER', 'talaria-pg-other'],
      ['TALARIA_REDIS_CONTAINER', 'talaria-redis-other'],
      ['TALARIA_PG_CONTAINER', 'devbox-b1-postgres'],
    ] as const) {
      const ctx = fakeCtx({ env: { [key]: value } })
      ctx.root = checkout('DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\n')
      const msg = attempt(() => devStack(ctx))
      expect(msg).toContain(`${key}=${value}`)
      expect(msg).toContain(`env -u ${key}`)
    }
    const empty = fakeCtx({ env: { TALARIA_PG_CONTAINER: '', TALARIA_REDIS_CONTAINER: 'talaria-redis-dev' } })
    empty.root = checkout('DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\n')
    expect(devStack(empty).pgContainer).toBe('talaria-postgres-dev')
    expect(devStack(empty).redisContainer).toBe('talaria-redis-dev')
  })

  test('a linked git worktree with no marker is refused: its stack is a guess', () => {
    const ctx = fakeCtx()
    ctx.root = checkout('DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\n', { linked: true })
    expect(attempt(() => devStack(ctx))).toContain('git worktree without an isolated stack')
  })

  test('a devbox is refused: the box compose owns its infra', () => {
    const ctx = fakeCtx()
    ctx.root = checkout('DATABASE_URL=postgres://talaria:talaria@postgres:5432/talaria\nTALARIA_DEVBOX=b1\n')
    expect(attempt(() => devStack(ctx))).toContain('devbox "b1"')
  })

  test("a shell TALARIA_WORKTREE dies, and the way out unsets everything that worktree's shell carries", () => {
    // A shell under a worktree's `talaria dev` holds its ui/.env too (vite
    // loads it into process.env). `unset TALARIA_WORKTREE` alone would pass
    // and leave the primary on the worktree's DATABASE_URL and fleet.
    const ctx = fakeCtx({
      env: {
        TALARIA_WORKTREE: 'demo',
        DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5605/talaria',
        TALARIA_FLEET_PROJECT: 'talaria-wt-demo-fleet',
        TALARIA_RUST_API_URL: 'http://127.0.0.1:5405',
        TALARIA_PG_CONTAINER: 'talaria-pg-demo',
        TALARIA_PG_PORT: '5605',
        HOME: '/home/someone',
      },
    })
    ctx.root = checkout('DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\n')
    const msg = attempt(() => devStack(ctx, undefined, 'reset database'))
    expect(msg).toContain('TALARIA_WORKTREE=demo in the environment')
    expect(msg).toContain('carries no worktree marker')
    expect(msg).toContain(
      'env -u TALARIA_WORKTREE -u TALARIA_PG_CONTAINER -u TALARIA_PG_PORT -u DATABASE_URL -u TALARIA_RUST_API_URL ' +
        '-u TALARIA_FLEET_PROJECT bun talaria reset database',
    )
    expect(msg).not.toContain('-u HOME')
  })
})

describe('verifyPostgres', () => {
  const wtStack: DevStack = {
    kind: 'worktree',
    worktree: 'demo',
    pgContainer: 'talaria-pg-demo',
    redisContainer: 'talaria-redis-demo',
    infraProject: 'talaria-wt-demo',
    fleetProject: 'talaria-wt-demo-fleet',
    fleetProblem: null,
  }
  const primary: DevStack = { ...wtStack, kind: 'primary', worktree: null, pgContainer: 'talaria-postgres-dev', infraProject: 'talaria-dev' }
  const plantPg = (ctx: FakeCtx, name: string, json: string) => ctx.plant(['docker', ['inspect', name]], json)

  test("the worktree's own container on DATABASE_URL's port passes", async () => {
    const ctx = fakeCtx()
    plantPg(ctx, 'talaria-pg-demo', inspectJson({ project: 'talaria-wt-demo', port: '5605' }))
    expect(await verifyPostgres(ctx, wtStack, 'postgres://talaria:talaria@127.0.0.1:5605/talaria')).toBe('5605')
  })

  test('a container of another compose project dies', async () => {
    const ctx = fakeCtx()
    plantPg(ctx, 'talaria-pg-demo', inspectJson({ project: 'talaria-dev', port: '5605' }))
    expect(await attemptAsync(() => verifyPostgres(ctx, wtStack, 'postgres://talaria:talaria@127.0.0.1:5605/talaria'))).toContain(
      "compose project 'talaria-dev'",
    )
  })

  test("a port that is not DATABASE_URL's dies", async () => {
    const ctx = fakeCtx()
    plantPg(ctx, 'talaria-pg-demo', inspectJson({ project: 'talaria-wt-demo', port: '5544' }))
    const msg = await attemptAsync(() => verifyPostgres(ctx, wtStack, 'postgres://talaria:talaria@127.0.0.1:5605/talaria'))
    expect(msg).toContain('DATABASE_URL names port 5605')
    expect(msg).toContain('publishes 5544')
  })

  test('not running, or unknown to docker, dies as not running', async () => {
    const stopped = fakeCtx()
    plantPg(stopped, 'talaria-pg-demo', inspectJson({ running: false, project: 'talaria-wt-demo', port: '5605' }))
    expect(await attemptAsync(() => verifyPostgres(stopped, wtStack, 'postgres://talaria:talaria@127.0.0.1:5605/talaria'))).toContain(
      'is not running',
    )
    const gone = fakeCtx()
    gone.plant(['docker', ['inspect', 'talaria-pg-demo']], new Error('No such object'))
    expect(await attemptAsync(() => verifyPostgres(gone, wtStack, 'postgres://talaria:talaria@127.0.0.1:5605/talaria'))).toContain(
      'is not running',
    )
  })

  test("a DATABASE_URL naming another database or role dies before docker is asked: psql runs as talaria on talaria", async () => {
    for (const url of [
      'postgres://talaria:talaria@127.0.0.1:5605/other',
      'postgres://bob:pw@127.0.0.1:5605/talaria',
      'postgres://127.0.0.1:5605',
      'not a url',
    ]) {
      const ctx = fakeCtx()
      plantPg(ctx, 'talaria-pg-demo', inspectJson({ project: 'talaria-wt-demo', port: '5605' }))
      expect(await attemptAsync(() => verifyPostgres(ctx, wtStack, url))).toContain('not the database DATABASE_URL names')
      expect(ctx.calls).toHaveLength(0)
    }
  })

  test("the primary's container passes on its port, and a worktree's container is refused there", async () => {
    const ok = fakeCtx()
    plantPg(ok, 'talaria-postgres-dev', inspectJson({ project: 'talaria-dev', port: '5544' }))
    expect(await verifyPostgres(ok, primary, 'postgres://talaria:talaria@127.0.0.1:5544/talaria')).toBe('5544')
    const theirs = fakeCtx()
    plantPg(theirs, 'talaria-postgres-dev', inspectJson({ project: 'talaria-wt-x', port: '5544' }))
    expect(await attemptAsync(() => verifyPostgres(theirs, primary, 'postgres://talaria:talaria@127.0.0.1:5544/talaria'))).toContain(
      "worktree project 'talaria-wt-x'",
    )
    const box = fakeCtx()
    plantPg(box, 'talaria-postgres-dev', inspectJson({ project: 'devbox-b1', port: '5544' }))
    expect(await attemptAsync(() => verifyPostgres(box, primary, 'postgres://talaria:talaria@127.0.0.1:5544/talaria'))).toContain(
      "devbox project 'devbox-b1'",
    )
  })
})

describe('refuseWorktreeEnvOverride', () => {
  const WT_UI = `${WT_ENV}REDIS_URL=redis://127.0.0.1:6505\nPORT=5305\n`
  const stackOf = (ctx: FakeCtx): DevStack => devStack(ctx, WT_UI)

  test('a shell value that is not ui/.env\'s dies, naming both with credentials cut', () => {
    const ctx = fakeCtx({ env: { DATABASE_URL: 'postgres://talaria:secret@127.0.0.1:5544/talaria', PORT: '5273' } })
    ctx.root = checkout(WT_UI, { linked: true })
    const msg = attempt(() => refuseWorktreeEnvOverride(ctx, stackOf(ctx), WT_UI, 'dev'))
    expect(msg).toContain('DATABASE_URL=postgres://127.0.0.1:5544/talaria   (ui/.env: postgres://127.0.0.1:5605/talaria)')
    expect(msg).toContain('PORT=5273   (ui/.env: 5305)')
    expect(msg).toContain('unset DATABASE_URL PORT')
    expect(msg).not.toContain('secret')
  })

  test("equal values, and lines ui/.env does not carry, are no conflict", () => {
    const ctx = fakeCtx({ env: { REDIS_URL: 'redis://127.0.0.1:6505', TALARIA_MCP_PORT: '5999' } })
    ctx.root = checkout(WT_UI, { linked: true })
    expect(attempt(() => refuseWorktreeEnvOverride(ctx, stackOf(ctx), WT_UI, 'dev'))).toBe('')
  })

  test('the primary keeps the shell as its override point', () => {
    const ctx = fakeCtx({ env: { DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5999/talaria' } })
    ctx.root = checkout('DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\n')
    const ui = 'DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\n'
    expect(attempt(() => refuseWorktreeEnvOverride(ctx, devStack(ctx, ui), ui, 'dev'))).toBe('')
  })

  test('printable cuts a url\'s userinfo and leaves anything else alone', () => {
    expect(printable('postgres://talaria:p@ss@127.0.0.1:5544/talaria')).toBe('postgres://127.0.0.1:5544/talaria')
    expect(printable('redis://:pw@127.0.0.1:6399')).toBe('redis://127.0.0.1:6399')
    expect(printable('redis://127.0.0.1:6399')).toBe('redis://127.0.0.1:6399')
    expect(printable('a@b')).toBe('a@b')
    expect(printable('5273')).toBe('5273')
  })
})

describe('projectContainers', () => {
  test('selects by the compose project label, never a name prefix', async () => {
    const ctx = fakeCtx()
    ctx.plant(
      ['docker', ['ps', '-a', '--filter', 'label=com.docker.compose.project=talaria-wt-demo-fleet', '--format', '{{.Names}}']],
      'talaria-wt-demo-fleet-agent-eng-1\n',
    )
    expect(await projectContainers(ctx, 'talaria-wt-demo-fleet', true)).toEqual(['talaria-wt-demo-fleet-agent-eng-1'])
    expect(ctx.calls[0]!.args).toContain('label=com.docker.compose.project=talaria-wt-demo-fleet')
  })

  test('a docker that refuses to list answers []', async () => {
    const ctx = fakeCtx()
    ctx.plant(['docker', ['ps', '--filter', 'label=com.docker.compose.project=p', '--format', '{{.Names}}']], new Error('daemon down'))
    expect(await projectContainers(ctx, 'p')).toEqual([])
  })
})

describe('refuseForeignCheckout', () => {
  test('standing in another checkout dies, naming both and the command that is not ambiguous', () => {
    const cliRoot = checkout('DATABASE_URL=x\n')
    const standing = checkout(WT_ENV, { linked: true })
    mkdirSync(join(standing, 'ui/src'), { recursive: true })
    const ctx = fakeCtx({ cwd: join(standing, 'ui/src') })
    ctx.root = cliRoot
    const msg = attempt(() => refuseForeignCheckout(ctx, 'reset database'))
    expect(msg).toContain(`runs the CLI of the checkout at ${cliRoot}`)
    expect(msg).toContain(`standing in ${standing}`)
    expect(msg).toContain(`cd ${standing} && bun talaria reset database`)
  })

  test("the CLI's own checkout, anywhere under it, passes", () => {
    const root = checkout('DATABASE_URL=x\n')
    mkdirSync(join(root, 'ui/src'), { recursive: true })
    const ctx = fakeCtx({ cwd: join(root, 'ui/src') })
    ctx.root = root
    expect(attempt(() => refuseForeignCheckout(ctx, 'dev'))).toBe('')
  })

  test('outside any checkout (cron at /, a home dir) passes', () => {
    const ctx = fakeCtx({ cwd: realpathSync(mkdtempSync(join(tmpdir(), 'talaria-nowhere-'))) })
    ctx.root = checkout('DATABASE_URL=x\n')
    expect(attempt(() => refuseForeignCheckout(ctx, 'backup'))).toBe('')
  })

  test("an app's own repo inside a checkout does not hide the checkout it sits in", () => {
    const cliRoot = checkout('DATABASE_URL=x\n')
    const standing = checkout(WT_ENV, { linked: true })
    mkdirSync(join(standing, 'apps/leadworks/.git'), { recursive: true })
    const ctx = fakeCtx({ cwd: join(standing, 'apps/leadworks') })
    ctx.root = cliRoot
    expect(checkoutAt(join(standing, 'apps/leadworks'))).toBe(standing)
    expect(attempt(() => refuseForeignCheckout(ctx, 'reset fleet'))).toContain(`standing in ${standing}`)
  })

  test('the two spellings of one directory are one checkout (a symlinked parent)', () => {
    const root = checkout('DATABASE_URL=x\n')
    const link = join(realpathSync(mkdtempSync(join(tmpdir(), 'talaria-link-'))), 'via')
    symlinkSync(root, link)
    const ctx = fakeCtx({ cwd: link })
    ctx.root = root
    expect(attempt(() => refuseForeignCheckout(ctx, 'dev'))).toBe('')
  })

  test('no cwd known: nothing to compare, passes', () => {
    const ctx = fakeCtx()
    expect(attempt(() => refuseForeignCheckout(ctx, 'dev'))).toBe('')
  })
})

describe('composeFileProject', () => {
  test("reads the renderer's top-level name, quoted or not", () => {
    expect(composeFileProject('# Generated\nname: talaria-wt-demo-fleet\nservices: {}\n')).toBe('talaria-wt-demo-fleet')
    expect(composeFileProject("name: 'talaria-fleet'\n")).toBe('talaria-fleet')
    expect(composeFileProject('services:\n  a:\n    name: nested\n')).toBeNull()
  })
})

describe('inside / physical', () => {
  test('a dir under the root, a dir beside it, and a symlinked spelling', () => {
    const root = realpathSync(mkdtempSync(join(tmpdir(), 'talaria-in-')))
    expect(inside(root, join(root, 'api/.uploads'))).toBe(true)
    expect(inside(root, root)).toBe(true)
    expect(inside(root, `${root}-other/app-data`)).toBe(false)
    const link = join(realpathSync(mkdtempSync(join(tmpdir(), 'talaria-in-link-'))), 'l')
    symlinkSync(root, link)
    expect(physical(join(link, 'not/yet/made'))).toBe(join(root, 'not/yet/made'))
    expect(inside(root, join(link, 'app-data'))).toBe(true)
  })
})

describe("main's containers named by the shell (box seed, box new, the primary's dev)", () => {
  // A worktree's `talaria dev` sets these four for its own compose call, in
  // its own process: everything it starts inherits them.
  const INHERITED = {
    TALARIA_PG_CONTAINER: 'talaria-pg-demo',
    TALARIA_PG_PORT: '5605',
    TALARIA_REDIS_CONTAINER: 'talaria-redis-demo',
    TALARIA_REDIS_PORT: '6505',
  }

  test("sideStackOfName knows a worktree's and a box's names, and nothing else", () => {
    expect(sideStackOfName('talaria-pg-demo')).toBe('worktree "demo"\'s postgres')
    expect(sideStackOfName('talaria-redis-fu-reset')).toBe('worktree "fu-reset"\'s redis')
    expect(sideStackOfName('talaria-wt-demo-fleet-agent-x-1')).toContain('talaria-wt-*')
    expect(sideStackOfName('devbox-b1-postgres')).toContain('devbox-*')
    for (const name of ['talaria-postgres-dev', 'talaria-storage-dev', 'talaria-qdrant-dev', 'talaria-embeddings-dev', 'my-pg', 'talaria-pg']) {
      expect(sideStackOfName(name)).toBeNull()
    }
    // the primary's own redis has a worktree's shape; sideStackContainers skips
    // the defaults for exactly that reason
    expect(sideStackOfName('talaria-redis-dev')).not.toBeNull()
  })

  test('isSideProject: a worktree\'s infra or fleet, or a box\'s; never main\'s', () => {
    for (const p of ['talaria-wt-demo', 'talaria-wt-demo-fleet', 'devbox-b1', 'devbox-b1-fleet']) expect(isSideProject(p)).toBe(true)
    for (const p of ['talaria-dev', 'talaria-fleet', 'talaria', '']) expect(isSideProject(p)).toBe(false)
  })

  test("primaryContainer: the shell's value, else the default; empty is unset; storage falls back to the MinIO name", () => {
    const none = fakeCtx()
    expect(primaryContainer(none, 'TALARIA_PG_CONTAINER')).toBe('talaria-postgres-dev')
    expect(primaryContainer(none, 'TALARIA_REDIS_CONTAINER')).toBe('talaria-redis-dev')
    expect(primaryContainer(none, 'TALARIA_QDRANT_CONTAINER')).toBe('talaria-qdrant-dev')
    expect(primaryContainer(none, 'TALARIA_EMBED_CONTAINER')).toBe('talaria-embeddings-dev')
    expect(primaryContainer(none, 'TALARIA_STORAGE_CONTAINER')).toBe('talaria-storage-dev')
    expect(primaryContainer(fakeCtx({ env: { TALARIA_PG_CONTAINER: '' } }), 'TALARIA_PG_CONTAINER')).toBe('talaria-postgres-dev')
    expect(primaryContainer(fakeCtx({ env: { TALARIA_PG_CONTAINER: 'my-pg' } }), 'TALARIA_PG_CONTAINER')).toBe('my-pg')
    expect(primaryContainer(fakeCtx({ env: { TALARIA_MINIO_CONTAINER: 'old-minio' } }), 'TALARIA_STORAGE_CONTAINER')).toBe('old-minio')
    expect(
      primaryContainer(fakeCtx({ env: { TALARIA_MINIO_CONTAINER: 'old-minio', TALARIA_STORAGE_CONTAINER: 'new-store' } }), 'TALARIA_STORAGE_CONTAINER'),
    ).toBe('new-store')
  })

  test('sideStackContainers is pure: every shaped override, never a default, never an unset key', () => {
    const ctx = fakeCtx({ env: { ...INHERITED, TALARIA_QDRANT_CONTAINER: 'devbox-b1-qdrant', TALARIA_EMBED_CONTAINER: 'my-tei' } })
    expect(sideStackContainers(ctx, ALL_PRIMARY_CONTAINER_VARS).map((f) => f.key)).toEqual([
      'TALARIA_PG_CONTAINER',
      'TALARIA_REDIS_CONTAINER',
      'TALARIA_QDRANT_CONTAINER',
    ])
    expect(sideStackContainers(fakeCtx({ env: { TALARIA_REDIS_CONTAINER: 'talaria-redis-dev' } }), ALL_PRIMARY_CONTAINER_VARS)).toEqual([])
    expect(sideStackContainers(fakeCtx(), ALL_PRIMARY_CONTAINER_VARS)).toEqual([])
    expect(ctx.calls).toHaveLength(0)
  })

  test("every line `talaria worktree` writes into ui/.env is one an inherited shell is told to unset", () => {
    for (const key of [...WORKTREE_ENV_KEYS, ...WORKTREE_OWN_VARS]) expect(WORKTREE_INHERITED_VARS).toContain(key)
  })

  test("the hint unsets the worktree's ui/.env lines the shell carries too: the primary's dev lets the shell win", async () => {
    const ctx = fakeCtx({
      env: {
        ...INHERITED,
        TALARIA_WORKTREE: 'demo',
        DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5605/talaria',
        TALARIA_API_PORT: '5405',
        HOME: '/home/someone',
      },
    })
    const msg = await attemptAsync(() => refuseSideStackContainers(ctx, ['TALARIA_PG_CONTAINER'], 'dev'))
    expect(msg).toContain(
      'env -u TALARIA_PG_CONTAINER -u TALARIA_PG_PORT -u TALARIA_REDIS_CONTAINER -u TALARIA_REDIS_PORT ' +
        '-u TALARIA_WORKTREE -u DATABASE_URL -u TALARIA_API_PORT bun talaria dev',
    )
    expect(msg).not.toContain('-u HOME')
    expect(ctx.calls).toHaveLength(0)
  })

  test("an inherited worktree container is refused by its shape, before docker is asked, and the hint unsets the ports too", async () => {
    const ctx = fakeCtx({ env: INHERITED })
    const msg = await attemptAsync(() => refuseSideStackContainers(ctx, ['TALARIA_PG_CONTAINER'], 'box seed demo'))
    expect(msg).toContain('TALARIA_PG_CONTAINER=talaria-pg-demo   (worktree "demo"\'s postgres)')
    expect(msg).toContain("A worktree's `talaria dev` sets these for itself")
    expect(msg).toContain(
      'env -u TALARIA_PG_CONTAINER -u TALARIA_PG_PORT -u TALARIA_REDIS_CONTAINER -u TALARIA_REDIS_PORT bun talaria box seed demo',
    )
    expect(ctx.calls).toHaveLength(0)
  })

  test("an override of any other name is asked of docker: a side project's container is refused, main's own passes", async () => {
    const theirs = fakeCtx({ env: { TALARIA_PG_CONTAINER: 'renamed-pg' } })
    theirs.plant(['docker', ['inspect', 'renamed-pg']], inspectJson({ project: 'talaria-wt-demo' }))
    expect(await attemptAsync(() => refuseSideStackContainers(theirs, ['TALARIA_PG_CONTAINER'], 'box new b1'))).toContain(
      'TALARIA_PG_CONTAINER=renamed-pg   (a container of compose project talaria-wt-demo)',
    )
    const box = fakeCtx({ env: { TALARIA_STORAGE_CONTAINER: 'some-store' } })
    box.plant(['docker', ['inspect', 'some-store']], inspectJson({ project: 'devbox-b1' }))
    expect(await attemptAsync(() => refuseSideStackContainers(box, ['TALARIA_STORAGE_CONTAINER'], 'dev'))).toContain(
      'compose project devbox-b1',
    )
    // main's own, renamed: honoured, as the primary's override point
    const mine = fakeCtx({ env: { TALARIA_PG_CONTAINER: 'my-main-pg' } })
    mine.plant(['docker', ['inspect', 'my-main-pg']], inspectJson({ project: 'talaria-dev' }))
    expect(await attemptAsync(() => refuseSideStackContainers(mine, ['TALARIA_PG_CONTAINER'], 'dev'))).toBe('')
    // a name docker does not know yet (compose will create it) passes the label half
    const fresh = fakeCtx({ env: { TALARIA_PG_CONTAINER: 'not-yet' } })
    fresh.plant(['docker', ['inspect', 'not-yet']], new Error('No such object'))
    expect(await attemptAsync(() => refuseSideStackContainers(fresh, ['TALARIA_PG_CONTAINER'], 'dev'))).toBe('')
  })

  test('only the keys asked about, and only the ones set, are judged', async () => {
    const ctx = fakeCtx({ env: { TALARIA_QDRANT_CONTAINER: 'devbox-b1-qdrant' } })
    expect(await attemptAsync(() => refuseSideStackContainers(ctx, ['TALARIA_PG_CONTAINER', 'TALARIA_STORAGE_CONTAINER'], 'box seed b2'))).toBe('')
    expect(ctx.calls).toHaveLength(0)
    expect(await attemptAsync(() => refuseSideStackContainers(ctx, ALL_PRIMARY_CONTAINER_VARS, 'box new b2'))).toContain(
      'TALARIA_QDRANT_CONTAINER=devbox-b1-qdrant',
    )
  })

  test('every offender is named in one refusal', async () => {
    const ctx = fakeCtx({ env: { ...INHERITED, TALARIA_MINIO_CONTAINER: 'devbox-b1-storage' } })
    const msg = await attemptAsync(() => refuseSideStackContainers(ctx, ALL_PRIMARY_CONTAINER_VARS, 'dev'))
    for (const key of ['TALARIA_PG_CONTAINER', 'TALARIA_REDIS_CONTAINER', 'TALARIA_MINIO_CONTAINER']) expect(msg).toContain(`${key}=`)
    expect(msg).toContain('-u TALARIA_MINIO_CONTAINER')
  })

  test("containerProject: the label, '' for none, null for a container docker does not know", async () => {
    const ctx = fakeCtx()
    ctx.plant(['docker', ['inspect', 'a']], inspectJson({ project: 'talaria-dev' }))
    ctx.plant(['docker', ['inspect', 'b']], inspectJson({}))
    ctx.plant(['docker', ['inspect', 'c']], new Error('No such object'))
    expect(await containerProject(ctx, 'a')).toBe('talaria-dev')
    expect(await containerProject(ctx, 'b')).toBe('')
    expect(await containerProject(ctx, 'c')).toBeNull()
  })
})
