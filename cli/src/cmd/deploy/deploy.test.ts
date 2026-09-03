// The deploy decision table: argv parity with docs/CONTAINER.md (the doc IS
// the contract — the up test reads it at runtime so the two can't drift
// apart silently), the DOCKER_GID resolution, the env-drift warning, and
// the creds fallback. Filesystem fixtures are real tmp trees; process
// behavior is planted.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  apiImageRef,
  credsCommand,
  downCommand,
  ensureSharedSecrets,
  logsCommand,
  runCreds,
  runDown,
  runLogs,
  runStatus,
  runUpdate,
  runUp,
  statusCommand,
  updateCommand,
  upCommand,
} from './actions'
import { deployCommand, warnEnvDrift } from './index'
import { fakeCtx, type FakeCtx } from '../../testing'
import { CliError } from '../../ui'
import type { Leaf, ParsedArgs } from '../../cli'

const NO_ARGS: ParsedArgs = { positionals: [], flags: {} }

/** A tmp repo with the parts of docker/ deploy reads: the compose fixture
 *  interpolates the vars the drift tests need, and the root Dockerfile
 *  carries the api-package ARG update pulls. CONTAINER.md parity comes
 *  from the REAL doc, not this fixture. */
const makeDeployTree = (envFile?: string) => {
  const root = mkdtempSync(join(tmpdir(), 'talaria-deploy-'))
  mkdirSync(join(root, 'docker'), { recursive: true })
  writeFileSync(
    join(root, 'docker/compose.yml'),
    [
      'services:',
      '  talaria:',
      `    group_add: ['\${DOCKER_GID:-999}']`,
      "    ports: ['\${TALARIA_HTTP_PORT:-5273}:5273']",
      '    environment:',
      '      TALARIA_STATE_DIR: ${TALARIA_STATE_DIR:-/var/lib/talaria}',
      '      TALARIA_FLEET_PROJECT: ${TALARIA_FLEET_PROJECT:-talaria-fleet}',
      '      TALARIA_FLEET_NETWORK: ${TALARIA_FLEET_NETWORK:-talaria}',
    ].join('\n'),
  )
  writeFileSync(join(root, 'Dockerfile'), 'ARG TALARIA_API_IMAGE=ghcr.io/outcrop-labs/talaria-api:main\n')
  if (envFile !== undefined) writeFileSync(join(root, 'docker/.env'), envFile)
  return root
}

/** The tokens after `docker` in CONTAINER.md's canonical up command — read
 *  from the doc at test time. Continuation lines are joined; the
 *  DOCKER_GID=$(stat …) prefix tokenizes harmlessly before the `docker`
 *  we slice from. */
const docUpArgv = (() => {
  const md = readFileSync(join(import.meta.dir, '../../../../docs/CONTAINER.md'), 'utf8')
  const fences = [...md.matchAll(/```bash\n([\s\S]*?)```/g)].map((m) => m[1]!)
  const block = fences.find((b) => b.includes('docker compose -f docker/compose.yml up'))
  if (!block) throw new Error('CONTAINER.md no longer contains the canonical up command')
  const tokens = block
    .replace(/\\\n/g, ' ')
    .split('\n')
    .flatMap((l) => l.split(/\s+/))
    .filter((t) => t.length > 0)
  const at = tokens.indexOf('docker')
  if (at === -1) throw new Error('CONTAINER.md up command has no docker token')
  return tokens.slice(at + 1)
})()

const attempt = async (fn: () => Promise<unknown>): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

describe('talaria deploy — group', () => {
  test('children: up, down, update, logs, creds, status', () => {
    expect(deployCommand.children.map((c) => c.name)).toEqual(['up', 'down', 'update', 'logs', 'creds', 'status'])
  })

  test('every leaf runs the drift warning first (the prelude wrapper)', async () => {
    const root = makeDeployTree()
    const ctx = fakeCtx({ env: { TALARIA_HTTP_PORT: '9999' } })
    ctx.root = root
    const status = deployCommand.children.find((c) => c.name === 'status')! as Leaf
    await status.run(ctx, NO_ARGS)
    expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes('TALARIA_HTTP_PORT'))).toBe(true)
  })
})

describe('talaria deploy up — argv parity with CONTAINER.md', () => {
  test('runs exactly the documented command, from the repo root', async () => {
    const root = makeDeployTree()
    const ctx = fakeCtx()
    ctx.root = root
    await runUp(ctx, '/nonexistent-deploy-test-socket') // deterministic: no GID injection
    const up = ctx.calls.find((c) => c.cmd === 'docker' && c.args.includes('up'))!
    expect(up.args).toEqual(docUpArgv)
    expect(up.args[0]).toBe('compose')
    expect(up.args[1]).toBe('-f')
    expect(up.args[2]).toBe('docker/compose.yml')
    expect(up.opts?.cwd).toBe(root)
    // the printed equivalent is the same command, copy-pasteable
    expect(ctx.logLines.some((l) => l.kind === 'say' && l.msg === 'docker compose -f docker/compose.yml up -d --build')).toBe(true)
  })

  test('DOCKER_GID resolved from the socket when nobody supplied one', async () => {
    const root = makeDeployTree()
    const sock = join(root, 'sock')
    writeFileSync(sock, '')
    const gid = String(statSync(sock).gid)
    const ctx = fakeCtx()
    ctx.root = root
    await runUp(ctx, sock)
    expect(ctx.env.DOCKER_GID).toBe(gid)
    expect(ctx.logLines.some((l) => l.kind === 'say' && l.msg.startsWith(`DOCKER_GID=${gid} docker compose`))).toBe(true)
  })

  test('a DOCKER_GID in docker/.env is honoured — the CLI injects nothing', async () => {
    const root = makeDeployTree('DOCKER_GID=42\n')
    const sock = join(root, 'sock') // a socket the CLI WOULD stat if it wrongly tried
    writeFileSync(sock, '')
    const ctx = fakeCtx()
    ctx.root = root
    await runUp(ctx, sock)
    expect(ctx.env.DOCKER_GID).toBeUndefined() // file value interpolates inside compose itself
    expect(ctx.logLines.some((l) => l.kind === 'say' && l.msg.startsWith('DOCKER_GID='))).toBe(false)
  })

  test('a shell-exported DOCKER_GID passes through untouched', async () => {
    const root = makeDeployTree()
    const sock = join(root, 'sock')
    writeFileSync(sock, '')
    const ctx = fakeCtx({ env: { DOCKER_GID: '42-from-shell' } })
    ctx.root = root
    await runUp(ctx, sock)
    expect(ctx.env.DOCKER_GID).toBe('42-from-shell')
    expect(ctx.logLines.some((l) => l.kind === 'say' && l.msg.startsWith('DOCKER_GID='))).toBe(false)
  })

  test('no socket → warn, no injection, compose default applies', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    await runUp(ctx, '/nonexistent-deploy-test-socket')
    expect(ctx.env.DOCKER_GID).toBeUndefined()
    expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes("couldn't stat"))).toBe(true)
  })
})

describe('talaria deploy — env drift', () => {
  test('exported + interpolated + absent from docker/.env → warned', () => {
    const ctx = fakeCtx({ env: { TALARIA_HTTP_PORT: '9999' } })
    ctx.root = makeDeployTree()
    warnEnvDrift(ctx)
    const warn = ctx.logLines.find((l) => l.kind === 'warn')
    expect(warn?.msg).toContain('TALARIA_HTTP_PORT')
    expect(warn?.msg).toContain('docker/.env')
  })

  test('present in docker/.env is not drift; uninterpolated exports are not drift', () => {
    const ctx = fakeCtx({ env: { DOCKER_GID: '42', TALARIA_DEVBOX: '1', TALARIA_SECRET_KEY: 'x' } })
    ctx.root = makeDeployTree('DOCKER_GID=42\n')
    warnEnvDrift(ctx)
    expect(ctx.logLines.filter((l) => l.kind === 'warn')).toHaveLength(0)
  })

  test('no docker/.env at all → every relevant export warns (that IS the trap)', () => {
    const ctx = fakeCtx({ env: { TALARIA_STATE_DIR: '/srv/tal' } })
    ctx.root = makeDeployTree()
    warnEnvDrift(ctx)
    expect(ctx.logLines.find((l) => l.kind === 'warn')?.msg).toContain('TALARIA_STATE_DIR')
  })
})

describe('talaria deploy down / logs / status / update', () => {
  test('down stops without touching volumes by default', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    await runDown(ctx, false)
    expect(ctx.calls.at(-1)!.args).toEqual(['compose', '-f', 'docker/compose.yml', 'down'])
  })

  test('down --volumes is explicit and the flag says what it costs', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    await downCommand.run(ctx, { positionals: [], flags: { volumes: true } })
    expect(ctx.calls.at(-1)!.args).toEqual(['compose', '-f', 'docker/compose.yml', 'down', '--volumes'])
    expect(downCommand.flags?.[0]!.desc).toContain('DATABASE')
  })

  test('logs follows every service', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    await runLogs(ctx)
    expect(ctx.calls.at(-1)!.args).toEqual(['compose', '-f', 'docker/compose.yml', 'logs', '-f'])
    expect(logsCommand.usage).toBe('talaria deploy logs')
  })

  test('status shows the effective port/state/fleet (env wins over docker/.env), then ps', async () => {
    const ctx = fakeCtx({ env: { TALARIA_HTTP_PORT: '9999' } })
    ctx.root = makeDeployTree('TALARIA_STATE_DIR=/srv/tal\n')
    await runStatus(ctx)
    const line = ctx.logLines.find((l) => l.kind === 'say' && l.msg.startsWith('http://'))!.msg
    expect(line).toContain('http://localhost:9999')
    expect(line).toContain('/srv/tal')
    expect(line).toContain('talaria-fleet/talaria') // the compose defaults
    expect(ctx.calls.at(-1)!.args).toEqual(['compose', '-f', 'docker/compose.yml', 'ps'])
  })

  test('update pulls --ff-only then runs the documented up', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    await runUpdate(ctx, '/nonexistent-deploy-test-socket')
    const pull = ctx.calls.find((c) => c.cmd === 'git')!
    expect(pull.args).toEqual(['pull', '--ff-only'])
    expect(pull.opts?.cwd).toBe(ctx.root)
    expect(ctx.calls.at(-1)!.args).toEqual(docUpArgv)
  })

  test('a failed pull dies before any docker command runs', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    ctx.plant(['git', ['pull', '--ff-only']], new Error('not possible'))
    const msg = await attempt(() => runUpdate(ctx))
    expect(msg).toContain('git pull failed')
    expect(ctx.calls.some((c) => c.cmd === 'docker')).toBe(false)
  })

  // bbills, 2026-09-03: `up -d --build` resolves the Dockerfile's api-package
  // FROM from the LOCAL daemon cache, so an update that skips the pull wraps
  // today's UI around yesterday's api binary — a redeploy that silently
  // changes nothing the api does.
  test('update pulls the api package the Dockerfile names, between the git pull and the build', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    await runUpdate(ctx, '/nonexistent-deploy-test-socket')
    const pkg = ctx.calls.find((c) => c.cmd === 'docker' && c.args[0] === 'pull')!
    expect(pkg.args).toEqual(['pull', 'ghcr.io/outcrop-labs/talaria-api:main'])
    // order: git pull → package pull → up --build
    const git = ctx.calls.findIndex((c) => c.cmd === 'git')
    const up = ctx.calls.findIndex((c) => c.args.includes('up'))
    expect(git).toBeLessThan(ctx.calls.indexOf(pkg))
    expect(ctx.calls.indexOf(pkg)).toBeLessThan(up)
    // printed like every other step — the copy-pasteable equivalent
    expect(ctx.logLines.some((l) => l.kind === 'say' && l.msg === 'docker pull ghcr.io/outcrop-labs/talaria-api:main')).toBe(true)
  })

  test('a failed package pull dies before the build — never bake the stale digest on purpose', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    ctx.plant(['docker', ['pull', 'ghcr.io/outcrop-labs/talaria-api:main']], new Error('no route to ghcr'))
    const msg = await attempt(() => runUpdate(ctx))
    expect(msg).toContain('stale package')
    expect(ctx.calls.some((c) => c.args.includes('up'))).toBe(false)
  })

  test('no api-package ARG in the Dockerfile → a skip note, straight to the documented up', async () => {
    const ctx = fakeCtx()
    const root = makeDeployTree()
    writeFileSync(join(root, 'Dockerfile'), 'FROM docker.io/library/node:22-alpine\n')
    ctx.root = root
    await runUpdate(ctx, '/nonexistent-deploy-test-socket')
    expect(ctx.calls.some((c) => c.cmd === 'docker' && c.args[0] === 'pull')).toBe(false)
    expect(ctx.calls.at(-1)!.args).toEqual(docUpArgv)
    expect(ctx.logLines.some((l) => l.kind === 'skip' && l.msg.includes('TALARIA_API_IMAGE'))).toBe(true)
    expect(apiImageRef(mkdtempSync(join(tmpdir(), 'talaria-empty-')))).toBeNull() // no Dockerfile at all
  })
})

describe('talaria deploy creds', () => {
  test('points at the claim screen — there is no generated password to print', async () => {
    const ctx = fakeCtx()
    ctx.root = makeDeployTree()
    expect(await runCreds(ctx)).toBe(0)
    const says = ctx.logLines.filter((l) => l.kind === 'say').map((l) => l.msg).join('\n')
    expect(says).toContain('http://localhost:5273')
    expect(says).toContain('claim')
    // No docker call, no secret surface — the command only prints a pointer.
    expect(ctx.calls.some((c) => c.cmd === 'docker')).toBe(false)
  })
})

describe('talaria deploy — leaves are honest about their usage strings', () => {
  test('up/down/update/logs/creds/status all declare usage', () => {
    for (const leaf of [upCommand, downCommand, updateCommand, logsCommand, credsCommand, statusCommand]) {
      expect(leaf.usage).toMatch(/^talaria deploy \w+/)
    }
  })
})

// #267 — the shared secrets. Compose interpolates POSTGRES_PASSWORD and the
// minio root pair into containers at CREATE time, before any entrypoint runs,
// so `deploy up` is the only moment anything can generate them: it writes
// docker/.env (the one file compose always reads) before invoking compose.
describe('talaria deploy up — first-boot shared secrets', () => {
  /** A ctx on a fresh host: the pg volume does not exist. fakeCtx's unplanted
   *  run() answers 0, so "fresh" must be planted explicitly. */
  const freshHost = (root: string) => {
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', ['volume', 'inspect', 'talaria_pg-data']], new Error('no such volume'))
    return ctx
  }

  test('a fresh host gets random secrets in docker/.env, once, 0600', async () => {
    const root = makeDeployTree()
    const ctx = freshHost(root)
    await runUp(ctx, '/nonexistent-deploy-test-socket')
    const envPath = join(root, 'docker/.env')
    const text = readFileSync(envPath, 'utf8')
    const pw = /POSTGRES_PASSWORD=([0-9a-f]+)/.exec(text)![1]!
    const ak = /TALARIA_S3_ACCESS_KEY=([0-9a-f]+)/.exec(text)![1]!
    const sk = /TALARIA_S3_SECRET_KEY=([0-9a-f]+)/.exec(text)![1]!
    expect(pw).toHaveLength(48) // randomBytes(24).hex — the documented openssl rand -hex 24
    expect(ak).not.toBe(sk) // independent values, not one blob twice
    expect((statSync(envPath).mode & 0o777)).toBe(0o600)
    // the story is names, never values
    const said = ctx.logLines.filter((l) => l.kind === 'say').map((l) => l.msg).join('\n')
    expect(said).toContain('POSTGRES_PASSWORD')
    expect(said).not.toContain(pw)
    // and the compose argv is still exactly the documented command
    expect(ctx.calls.find((c) => c.cmd === 'docker' && c.args.includes('up'))!.args).toEqual(docUpArgv)
  })

  test('operator-supplied values win — shell env and existing file lines are never overwritten', async () => {
    const root = makeDeployTree('POSTGRES_PASSWORD=mine-already\nDOCKER_GID=7\n')
    const ctx = fakeCtx({ env: { TALARIA_S3_SECRET_KEY: 'from-shell' } })
    ctx.root = root
    await ensureSharedSecrets(ctx, 'talaria_pg-data')
    const text = readFileSync(join(root, 'docker/.env'), 'utf8')
    expect(text).toContain('POSTGRES_PASSWORD=mine-already')
    expect(text).toContain('DOCKER_GID=7') // pre-existing lines ride along untouched
    expect(text).not.toContain('from-shell') // shell values stay in the shell
    expect(text).toMatch(/^TALARIA_S3_ACCESS_KEY=[0-9a-f]{48}$/m) // only the gap is filled
    expect(text).not.toMatch(/^TALARIA_S3_SECRET_KEY=/m)
  })

  test('existing postgres data keeps the password it was born with — a fresh random would lock the app out', async () => {
    const root = makeDeployTree()
    const ctx = fakeCtx() // unplanted run() answers 0: the volume exists
    ctx.root = root
    await ensureSharedSecrets(ctx)
    const text = readFileSync(join(root, 'docker/.env'), 'utf8')
    expect(text).toContain('POSTGRES_PASSWORD=talaria') // the published default, pinned not randomized
    expect(text).toContain('rotate') // the file says how to leave the default behind
    const warn = ctx.logLines.find((l) => l.kind === 'warn')!
    expect(warn.msg).toContain('rotate')
    // minio's pair is env-driven at every boot — it still rotates freely
    expect(text).toMatch(/^TALARIA_S3_SECRET_KEY=[0-9a-f]{48}$/m)
  })

  test('idempotent: the second up changes nothing and says nothing', async () => {
    const root = makeDeployTree()
    const first = freshHost(root)
    await ensureSharedSecrets(first)
    const after = readFileSync(join(root, 'docker/.env'), 'utf8')
    const second = freshHost(root)
    await ensureSharedSecrets(second)
    expect(readFileSync(join(root, 'docker/.env'), 'utf8')).toBe(after)
    expect(second.logLines.filter((l) => l.kind === 'say' && l.msg.includes('generated'))).toHaveLength(0)
  })
})
