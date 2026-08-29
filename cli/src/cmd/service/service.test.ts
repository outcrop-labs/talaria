// The service decision table: the pure pieces (unit text, sudo spelling,
// PATH walk, show parsing, version gating, DOCKER_GID pinning) and the three
// orchestrations against planted answers. HostPaths/euid/platform are
// injected everywhere, so nothing here touches the real /etc, /run, or sudo.

import { describe, expect, test } from 'bun:test'
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { runInstall, withDockerGid } from './install'
import { runUninstall } from './uninstall'
import { runServiceStatus } from './status'
import { serviceCommand } from './index'
import { unitText } from './unit'
import {
  parseComposeVersion,
  parseShow,
  privileged,
  resolveBin,
  upArgsFor,
  type HostPaths,
  unitPath,
} from './shared'
import { fakeCtx } from '../../testing'
import { CliError } from '../../ui'
import type { Leaf } from '../../cli'

const attempt = async (fn: () => unknown): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

/** A tmp repo with the parts of docker/ install reads (compose interpolates
 *  the drift vars; docker/.env optional). */
const makeRepo = (envFile?: string) => {
  const root = mkdtempSync(join(tmpdir(), 'talaria-repo-'))
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
    ].join('\n'),
  )
  if (envFile !== undefined) writeFileSync(join(root, 'docker/.env'), envFile)
  return root
}

/** A tmp host: an /etc/systemd stand-in (unit existence) and the
 *  /run/systemd/system marker (the "systemd is the manager" probe). */
const makeHost = () => {
  const base = mkdtempSync(join(tmpdir(), 'talaria-host-'))
  const host: HostPaths = { systemdDir: join(base, 'etc-systemd'), runSystemd: join(base, 'run-systemd') }
  mkdirSync(host.systemdDir, { recursive: true })
  mkdirSync(host.runSystemd, { recursive: true })
  return { base, host }
}

/** Executable systemctl/sudo/docker stubs on a PATH the tests control. */
const makeStubs = (bins: string[] = ['systemctl', 'sudo', 'docker']) => {
  const dir = mkdtempSync(join(tmpdir(), 'talaria-stubs-'))
  for (const b of bins) {
    writeFileSync(join(dir, b), '#!/bin/sh\n')
    chmodSync(join(dir, b), 0o755)
  }
  return dir
}

const DOCKER_SERVICE_SHOW = ['show', 'docker.service', '-p', 'LoadState', '-p', 'UnitFileState'] as const

describe('service shared — pure pieces', () => {
  test('upArgsFor gates --wait on compose 2.24 (the one-shot hang)', () => {
    expect(upArgsFor('2.24.0')).toEqual(['up', '-d', '--wait'])
    expect(upArgsFor('2.23.9')).toEqual(['up', '-d'])
    expect(upArgsFor('2.29.7')).toEqual(['up', '-d', '--wait'])
    expect(upArgsFor('5.4.0')).toEqual(['up', '-d', '--wait'])
    expect(upArgsFor(null)).toEqual(['up', '-d'])
  })

  test('parseComposeVersion reads the spellings compose actually prints', () => {
    expect(parseComposeVersion('5.4.0\n')).toBe('5.4.0')
    expect(parseComposeVersion('v2.29.7-desktop.2\n')).toBe('2.29.7')
    expect(parseComposeVersion('Docker Compose version v2.27.0\n')).toBe('2.27.0')
    expect(parseComposeVersion('garbage')).toBeNull()
    expect(parseComposeVersion('')).toBeNull()
  })

  test('privileged: bare as root, sudo-wrapped otherwise', () => {
    expect(privileged(0, 'systemctl', ['daemon-reload'])).toEqual(['systemctl', ['daemon-reload']])
    expect(privileged(1000, 'systemctl', ['daemon-reload'])).toEqual(['sudo', ['systemctl', 'daemon-reload']])
  })

  test('resolveBin walks PATH for executables only', () => {
    const dir = makeStubs(['systemctl'])
    writeFileSync(join(dir, 'plain'), 'x') // present but not executable
    expect(resolveBin('systemctl', { PATH: dir })).toBe(join(dir, 'systemctl'))
    expect(resolveBin('plain', { PATH: dir })).toBeNull()
    expect(resolveBin('absent', { PATH: dir })).toBeNull()
    expect(resolveBin('systemctl', {})).toBeNull()
  })

  test('parseShow reads Key=Value lines', () => {
    expect(parseShow('LoadState=loaded\nUnitFileState=enabled\n')).toEqual({ LoadState: 'loaded', UnitFileState: 'enabled' })
    expect(parseShow('')).toEqual({})
  })

  test('withDockerGid appends under a dated comment, or returns the input identical', () => {
    expect(withDockerGid('', '42', '2026-01-01')).toBe(
      '# pinned by `talaria service install` 2026-01-01 — the boot unit interpolates from this file\nDOCKER_GID=42\n',
    )
    const populated = withDockerGid('A=1\n', '42', '2026-01-01')
    expect(populated).toContain('A=1\n\n# pinned')
    expect(populated).toContain('DOCKER_GID=42\n')
    const already = 'DOCKER_GID=7\nA=1\n'
    expect(withDockerGid(already, '42', '2026-01-01')).toBe(already)
  })
})

describe('unitText — the unit file', () => {
  const text = unitText({ root: '/repo', dockerBin: '/usr/bin/docker', upArgs: ['up', '-d', '--wait'] })

  test('the load-bearing lines', () => {
    expect(text).toContain('Type=oneshot\n')
    expect(text).toContain('RemainAfterExit=yes\n')
    expect(text).toContain('Requires=docker.service\n')
    expect(text).toContain('After=docker.service network-online.target\n')
    expect(text).toContain('WantedBy=multi-user.target\n')
    expect(text).toContain('WorkingDirectory=/repo\n')
    expect(text).toContain('Documentation=file:///repo/docs/CONTAINER.md\n')
  })

  test('ExecStart/ExecStop: absolute docker, the documented relative -f, no --build, no --volumes', () => {
    expect(text).toContain('ExecStart=/usr/bin/docker compose -f docker/compose.yml up -d --wait\n')
    expect(text).toContain('ExecStop=/usr/bin/docker compose -f docker/compose.yml down\n')
    // the comments above the directives EXPLAIN the absences — only the
    // directive lines themselves must be clean
    const execLines = text.split('\n').filter((l) => l.startsWith('Exec'))
    expect(execLines.every((l) => !l.includes('--build') && !l.includes('--volumes'))).toBe(true)
  })

  test('Restart=on-failure only — systemd rejects always for a oneshot; --wait drops on old compose', () => {
    expect(text).toContain('Restart=on-failure\n')
    expect(text.split('\n').some((l) => l.startsWith('Restart=always'))).toBe(false)
    const old = unitText({ root: '/repo', dockerBin: '/usr/bin/docker', upArgs: upArgsFor('2.20.1') })
    expect(old.split('\n').find((l) => l.startsWith('ExecStart'))).toBe(
      'ExecStart=/usr/bin/docker compose -f docker/compose.yml up -d',
    )
  })
})

describe('talaria service install — orchestration', () => {
  test('happy path over sudo: pins DOCKER_GID, ups the stack, installs the unit, enables it, cleans up', async () => {
    const root = makeRepo()
    const { base, host } = makeHost()
    const stubs = makeStubs()
    const sock = join(base, 'docker.sock')
    writeFileSync(sock, '')
    const gid = String(statSync(sock).gid)
    const ctx = fakeCtx({ env: { PATH: stubs } })
    ctx.root = root
    ctx.plant(['systemctl', [...DOCKER_SERVICE_SHOW]], 'LoadState=loaded\nUnitFileState=enabled\n')
    ctx.plant(['docker', ['compose', 'version', '--short']], 'v2.29.7\n')

    expect(await runInstall(ctx, { host, euid: 1000, sock })).toBe(0)

    // DOCKER_GID pinned, 0600, and deploy's up saw no reason to inject one
    const envFile = join(root, 'docker/.env')
    expect(readFileSync(envFile, 'utf8')).toContain(`DOCKER_GID=${gid}\n`)
    expect(statSync(envFile).mode & 0o777).toBe(0o600)

    // order: deploy's up (build belongs to the operator's terminal), then the privileged trio
    const cmds = ctx.calls.map((c) => [c.cmd, ...c.args].join(' '))
    const upAt = cmds.findIndex((c) => c.endsWith('compose -f docker/compose.yml up -d --build'))
    const installAt = cmds.findIndex((c) => c.startsWith('sudo install -m 0644'))
    const reloadAt = cmds.indexOf('sudo systemctl daemon-reload')
    const enableAt = cmds.indexOf('sudo systemctl enable --now talaria.service')
    expect(upAt).toBeGreaterThanOrEqual(0)
    expect(installAt).toBeGreaterThan(upAt)
    expect(reloadAt).toBeGreaterThan(installAt)
    expect(enableAt).toBeGreaterThan(reloadAt)

    // the staged unit was a temp file, and it is gone afterwards
    const staged = ctx.calls.find((c) => c.cmd === 'sudo' && c.args[0] === 'install')!.args[3]!
    expect(staged).toMatch(/talaria-unit-.*talaria\.service$/)
    expect(existsSync(dirname(staged))).toBe(false)

    // the staged text is the rendered unit, with --wait from the planted version
    expect(ctx.logLines.some((l) => l.kind === 'raw' && l.msg.includes('up -d --wait'))).toBe(true)
  })

  test('as root (euid 0) the same steps run bare — no sudo token anywhere', async () => {
    const root = makeRepo()
    const { base, host } = makeHost()
    const stubs = makeStubs()
    const ctx = fakeCtx({ env: { PATH: stubs } })
    ctx.root = root
    ctx.plant(['systemctl', [...DOCKER_SERVICE_SHOW]], 'LoadState=loaded\nUnitFileState=enabled\n')
    ctx.plant(['docker', ['compose', 'version', '--short']], '5.4.0\n')
    await runInstall(ctx, { host, euid: 0, sock: join(base, 'docker.sock') })
    expect(ctx.calls.some((c) => c.cmd === 'sudo')).toBe(false)
    expect(ctx.calls.some((c) => c.cmd === 'systemctl' && c.args.join(' ') === 'enable --now talaria.service')).toBe(true)
    expect(ctx.calls.some((c) => c.cmd === 'install' && c.args[0] === '-m')).toBe(true)
  })

  test('stack already running → deploy up is skipped, enable --now still runs', async () => {
    const root = makeRepo()
    const { host } = makeHost()
    const ctx = fakeCtx({ env: { PATH: makeStubs() } })
    ctx.root = root
    ctx.plant(['systemctl', [...DOCKER_SERVICE_SHOW]], 'LoadState=loaded\nUnitFileState=enabled\n')
    ctx.plant(['docker', ['compose', 'version', '--short']], 'v2.29.7\n')
    ctx.plant(['docker', ['compose', '-f', 'docker/compose.yml', 'ps', '--quiet', '--status', 'running']], 'abc123\n')
    await runInstall(ctx, { host, euid: 1000 })
    expect(ctx.calls.some((c) => c.args.includes('up'))).toBe(false)
    expect(ctx.logLines.some((l) => l.kind === 'skip' && l.msg.includes('already running'))).toBe(true)
    expect(ctx.calls.some((c) => c.cmd === 'sudo' && c.args[1] === 'enable')).toBe(true)
  })

  test('DOCKER_GID precedence: file wins unpinned, exported value lands in the file, socket fills the gap', async () => {
    // "Untouched" stopped being a file-level invariant when #267 made `up`
    // append the shared secrets to this same docker/.env — the GID block's
    // promise is narrower and still holds: a value already there is never
    // re-pinned, never rewritten.
    const cases: { envFile?: string; env?: Record<string, string>; expectGid?: string; untouched?: boolean }[] = [
      { envFile: 'DOCKER_GID=42\n', expectGid: '42', untouched: true },
      { env: { DOCKER_GID: '7-from-shell' }, expectGid: '7-from-shell' },
    ]
    for (const c of cases) {
      const root = makeRepo(c.envFile)
      const { base, host } = makeHost()
      const sock = join(base, 'docker.sock')
      writeFileSync(sock, '')
      const before = c.envFile ?? ''
      const ctx = fakeCtx({ env: { PATH: makeStubs(), ...c.env } })
      ctx.root = root
      ctx.plant(['systemctl', [...DOCKER_SERVICE_SHOW]], 'LoadState=loaded\nUnitFileState=enabled\n')
      await runInstall(ctx, { host, euid: 1000, sock })
      const after = readFileSync(join(root, 'docker/.env'), 'utf8')
      expect(after).toContain(`DOCKER_GID=${c.expectGid}`)
      if (c.untouched) {
        expect(after.startsWith(before)).toBe(true)
        expect(after).not.toContain('pinned by `talaria service install`')
      }
    }

    // neither file nor shell: the socket's gid is pinned
    const root = makeRepo()
    const { base, host } = makeHost()
    const sock = join(base, 'docker.sock')
    writeFileSync(sock, '')
    const ctx = fakeCtx({ env: { PATH: makeStubs() } })
    ctx.root = root
    ctx.plant(['systemctl', [...DOCKER_SERVICE_SHOW]], 'LoadState=loaded\nUnitFileState=enabled\n')
    await runInstall(ctx, { host, euid: 1000, sock })
    expect(readFileSync(join(root, 'docker/.env'), 'utf8')).toContain(`DOCKER_GID=${statSync(sock).gid}\n`)
  })

  test('no socket anywhere → warn, nothing pinned', async () => {
    const root = makeRepo()
    const { host } = makeHost()
    const ctx = fakeCtx({ env: { PATH: makeStubs() } })
    ctx.root = root
    ctx.plant(['systemctl', [...DOCKER_SERVICE_SHOW]], 'LoadState=loaded\nUnitFileState=enabled\n')
    await runInstall(ctx, { host, euid: 1000, sock: '/nonexistent-service-test-sock' })
    expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes("couldn't stat"))).toBe(true)
    // docker/.env may exist after this — #267's shared secrets create it at
    // up-time — but a socket nobody could stat must pin no GID into it.
    const envFile = join(root, 'docker/.env')
    if (existsSync(envFile)) expect(readFileSync(envFile, 'utf8')).not.toContain('DOCKER_GID=')
  })

  test('guards die before anything privileged runs', async () => {
    const { host } = makeHost()
    const stubs = makeStubs()

    const guard = async (opts: {
      env?: Record<string, string>
      platform?: string
      stubs?: string
      host?: HostPaths
      root?: () => string
      plant?: (ctx: ReturnType<typeof fakeCtx>) => void
      expect: string
    }): Promise<void> => {
      const ctx = fakeCtx({ env: { PATH: opts.stubs ?? stubs, ...opts.env } })
      ctx.root = opts.root ? opts.root() : makeRepo()
      opts.plant?.(ctx)
      const msg = await attempt(() =>
        runInstall(ctx, { host: opts.host ?? host, euid: 1000, platform: opts.platform, sock: '/nonexistent-service-test-sock' }),
      )
      expect(msg).toContain(opts.expect)
      expect(ctx.calls.some((c) => c.cmd === 'sudo' || c.cmd === 'install')).toBe(false)
    }

    await guard({ platform: 'darwin', expect: 'Linux/systemd' })
    await guard({ env: { TALARIA_DEVBOX: 'demo' }, expect: 'devbox' })
    await guard({ stubs: makeStubs(['docker', 'sudo']), expect: 'systemctl not found' })
    await guard({ host: { systemdDir: host.systemdDir, runSystemd: '/nonexistent-run-systemd' }, expect: 'not the running manager' })
    await guard({
      root: () => {
        const bare = mkdtempSync(join(tmpdir(), 'talaria-bare-'))
        return bare
      },
      expect: 'deploy checkout',
    })
    await guard({
      plant: (ctx) => {
        ctx.plant(['docker', ['--version']], new Error('no docker'))
      },
      expect: 'docker is required',
    })
    await guard({
      plant: (ctx) => {
        ctx.plant(['systemctl', [...DOCKER_SERVICE_SHOW]], 'LoadState=masked\nUnitFileState=masked\n')
      },
      expect: 'masked',
    })
  })

  test('no sudo and not root → dies with the copy-pasteable commands, staging dir kept', async () => {
    const root = makeRepo()
    const { host } = makeHost()
    const ctx = fakeCtx({ env: { PATH: makeStubs(['systemctl', 'docker']) } })
    ctx.root = root
    ctx.plant(['systemctl', [...DOCKER_SERVICE_SHOW]], 'LoadState=loaded\nUnitFileState=enabled\n')
    const msg = await attempt(() => runInstall(ctx, { host, euid: 1000 }))
    expect(msg).toContain('sudo not found')
    expect(msg).toContain('systemctl enable --now talaria.service')
    const staged = msg.match(/staged at (\S+)/)![1]!
    expect(existsSync(staged)).toBe(true) // left in place on purpose — the printed commands reference it
  })
})

describe('talaria service uninstall', () => {
  test('disable --now (the stack goes down), rm the unit, reload, reset-failed — volumes never touched', async () => {
    const { host } = makeHost()
    writeFileSync(unitPath(host), '# unit\n')
    const ctx = fakeCtx({ env: { PATH: makeStubs() } })
    ctx.root = makeRepo()
    expect(await runUninstall(ctx, { host, euid: 1000 })).toBe(0)
    expect(ctx.calls.map((c) => [c.cmd, ...c.args].join(' '))).toEqual([
      'sudo systemctl disable --now talaria.service',
      `sudo rm -f ${unitPath(host)}`,
      'sudo systemctl daemon-reload',
      'sudo systemctl reset-failed talaria.service',
    ])
    expect(ctx.calls.some((c) => c.args.includes('--volumes'))).toBe(false)
  })

  test('nothing installed → dies pointing at install', async () => {
    const { host } = makeHost()
    const ctx = fakeCtx()
    const msg = await attempt(() => runUninstall(ctx, { host, euid: 0 }))
    expect(msg).toContain('not installed')
  })
})

describe('talaria service status', () => {
  test('unit state lines, then deploy\'s compose view', async () => {
    const { host } = makeHost()
    writeFileSync(unitPath(host), '# unit\n')
    const root = makeRepo()
    const ctx = fakeCtx({ env: { PATH: makeStubs() } })
    ctx.root = root
    ctx.plant(
      ['systemctl', ['show', 'talaria.service', '-p', 'LoadState', '-p', 'UnitFileState', '-p', 'ActiveState', '-p', 'SubState', '-p', 'ExecMainStatus']],
      'LoadState=loaded\nUnitFileState=enabled\nActiveState=active\nSubState=exited\nExecMainStatus=0\n',
    )
    expect(await runServiceStatus(ctx, { host })).toBe(0)
    expect(ctx.logLines.some((l) => l.kind === 'say' && l.msg.includes('enabled'))).toBe(true)
    expect(ctx.logLines.some((l) => l.kind === 'say' && l.msg.includes('active'))).toBe(true)
    expect(ctx.calls.at(-1)!.args).toEqual(['compose', '-f', 'docker/compose.yml', 'ps'])
  })

  test('nothing installed → dies pointing at install', async () => {
    const { host } = makeHost()
    const msg = await attempt(() => runServiceStatus(fakeCtx(), { host }))
    expect(msg).toContain('not installed')
  })
})

describe('talaria service — group', () => {
  test('children: install, uninstall, status; usage strings are honest', () => {
    expect(serviceCommand.children.map((c) => c.name)).toEqual(['install', 'uninstall', 'status'])
    for (const leaf of serviceCommand.children) {
      expect((leaf as Leaf).usage).toMatch(/^talaria service \w+$/)
    }
  })

  test('every leaf runs the drift warning first (the prelude wrapper)', async () => {
    const root = makeRepo() // interpolates TALARIA_HTTP_PORT; no docker/.env
    const ctx = fakeCtx({ env: { PATH: makeStubs(), TALARIA_HTTP_PORT: '9999' } })
    ctx.root = root
    ctx.plant(
      ['systemctl', ['show', 'talaria.service', '-p', 'LoadState', '-p', 'UnitFileState', '-p', 'ActiveState', '-p', 'SubState', '-p', 'ExecMainStatus']],
      'LoadState=loaded\nUnitFileState=enabled\nActiveState=active\nSubState=exited\nExecMainStatus=0\n',
    )
    const status = serviceCommand.children.find((c) => c.name === 'status')! as Leaf
    // The leaf takes no host injection — it may die at the real-host unit
    // check; the point is what ran BEFORE that, and nothing privileged ever.
    await attempt(() => status.run(ctx, { positionals: [], flags: {} }))
    expect(ctx.logLines.some((l) => l.kind === 'warn' && l.msg.includes('TALARIA_HTTP_PORT'))).toBe(true)
    expect(ctx.calls.some((c) => c.cmd === 'sudo')).toBe(false)
  })
})
