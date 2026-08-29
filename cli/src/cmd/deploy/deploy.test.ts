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
  credsCommand,
  downCommand,
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

/** A tmp repo with the parts of docker/ deploy reads. The compose fixture
 *  interpolates the vars the drift tests need; CONTAINER.md parity comes
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
