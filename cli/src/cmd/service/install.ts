// `talaria service install` — start production and keep it running across
// reboots. Plain compose (`talaria deploy up`) stays the canonical path;
// this adds the one thing it can't give itself: a systemd unit that starts
// the stack at boot, health-gates that start, and stops it cleanly at
// shutdown. Full story: docs/CONTAINER.md → "Keep it running across
// reboots".
//
// Everything privileged is sudo-wrapped (bare when already root), printed
// before it runs, and runs through ctx.run so sudo can prompt on the
// inherited tty — the same honest-wrapper contract as `talaria deploy`.

import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { dockerEnvFile, runUp, socketGid } from '../deploy/actions'
import {
  COMPOSE_FILE,
  HOST,
  UNIT_NAME,
  type HostPaths,
  parseComposeVersion,
  privileged,
  resolveBin,
  unitPath,
  unitState,
  upArgsFor,
} from './shared'
import { unitText } from './unit'

const DOCKER_SOCK = '/var/run/docker.sock'

/** docker/.env with DOCKER_GID pinned: appended under a dated comment, or
 *  the input byte-identical when the key is already there. The dated comment
 *  is for the operator reading the file later — the CLI never re-reads it. */
export function withDockerGid(existing: string, gid: string, date: string): string {
  if (/^DOCKER_GID=/m.test(existing)) return existing
  const body = existing.replace(/\s+$/, '')
  return `${body === '' ? '' : `${body}\n\n`}# pinned by \`talaria service install\` ${date} — the boot unit interpolates from this file\nDOCKER_GID=${gid}\n`
}

export type InstallOpts = { host?: HostPaths; euid?: number; platform?: string; sock?: string }

export async function runInstall(ctx: Ctx, o: InstallOpts = {}): Promise<number> {
  const host = o.host ?? HOST
  const euid = o.euid ?? process.getuid?.() ?? 1000
  const platform = o.platform ?? process.platform
  const sock = o.sock ?? DOCKER_SOCK

  // Guards — each dies before anything privileged happens.
  if (platform !== 'linux') {
    ctx.log.die('service management is Linux/systemd — elsewhere the stack survives reboots on its own once docker starts (restart: unless-stopped; docs/CONTAINER.md)')
  }
  if (ctx.env.TALARIA_DEVBOX) ctx.log.die('inside a devbox — `service install` manages the host, not a box')
  if (resolveBin('systemctl', ctx.env) === null) {
    ctx.log.die('systemctl not found — not a systemd host? Without a unit the stack still restarts via restart: unless-stopped (docs/CONTAINER.md)')
  }
  if (!existsSync(host.runSystemd)) {
    ctx.log.die('systemd is not the running manager (a container, or WSL without it — /etc/wsl.conf `[boot] systemd=true`), so `systemctl enable --now` cannot work here')
  }
  try {
    await ctx.exec('docker', ['--version'])
  } catch {
    ctx.log.die('docker is required')
  }
  if (!existsSync(join(ctx.root, COMPOSE_FILE))) {
    ctx.log.die(`no ${COMPOSE_FILE} here — \`service install\` runs from a deploy checkout`)
  }

  // The unit's up argv: --wait only on a compose new enough not to hang on
  // the one-shot init container (see shared.upArgsFor).
  let versionOut: string
  try {
    versionOut = (await ctx.exec('docker', ['compose', 'version', '--short'])).stdout
  } catch {
    ctx.log.die('`docker compose version` failed — the compose plugin is required (docs/CONTAINER.md)')
  }
  const version = parseComposeVersion(versionOut)
  const upArgs = upArgsFor(version)
  ctx.log.say(
    upArgs.includes('--wait')
      ? `compose ${version} → the unit gates its start on the stack's healthchecks (up -d --wait)`
      : `compose ${version ?? '?'} → the unit starts plain (up -d); this build hangs on --wait with the one-shot init container`,
  )

  // Boot ordering needs docker.service pullable. Masked is fatal; disabled
  // is fine — Requires= pulls a disabled unit in whenever talaria starts.
  const dockerUnit = await unitState(ctx, 'docker.service', ['LoadState', 'UnitFileState'])
  if (dockerUnit.LoadState === 'masked') {
    ctx.log.die('docker.service is masked — unmask it first (`sudo systemctl unmask docker.service`); a masked unit cannot be pulled in at boot')
  }
  if (dockerUnit.LoadState === 'not-found') {
    ctx.log.warn('no docker.service unit (rootless or desktop docker?) — the installed unit assumes the system daemon on /var/run/docker.sock')
  } else if (dockerUnit.UnitFileState === 'disabled') {
    ctx.log.say('docker.service is disabled — Requires= still starts it whenever talaria starts')
  } else if (dockerUnit.UnitFileState) {
    ctx.log.ok(`docker.service ${dockerUnit.UnitFileState}`)
  } else {
    ctx.log.warn('could not read docker.service state — continuing')
  }

  // The boot unit has no shell: whatever DOCKER_GID deploy up would resolve
  // into its process env must live in docker/.env, the only file compose
  // interpolates from in both worlds. A value already there wins.
  const envFile = join(ctx.root, 'docker/.env')
  if (dockerEnvFile(ctx).DOCKER_GID !== undefined) {
    ctx.log.skip('DOCKER_GID already pinned in docker/.env')
  } else {
    const gid = ctx.env.DOCKER_GID ?? socketGid(sock)
    if (gid === null) {
      ctx.log.warn(`couldn't stat ${sock} — DOCKER_GID stays at compose's default (999)`)
    } else {
      const existing = existsSync(envFile) ? readFileSync(envFile, 'utf8') : ''
      writeFileSync(envFile, withDockerGid(existing, gid, ctx.now().toISOString().slice(0, 10)), { mode: 0o600 })
      ctx.log.ok(`DOCKER_GID=${gid} pinned into docker/.env — the unit's up interpolates from that file`)
    }
  }
  if (ctx.env.DOCKER_HOST) {
    ctx.log.warn('DOCKER_HOST is set in this shell — the unit talks to the default socket as root, not this context')
  }

  // The "start it in production" half: bring the stack up the deploy way if
  // it isn't already — build failures belong in the operator's terminal, not
  // in journald's first boot.
  let running = ''
  try {
    running = (await ctx.exec('docker', ['compose', '-f', COMPOSE_FILE, 'ps', '--quiet', '--status', 'running'], { cwd: ctx.root })).stdout.trim()
  } catch {
    // compose too old for --status — treat as not running and let up decide
  }
  if (running !== '') {
    ctx.log.skip('stack already running')
  } else if ((await runUp(ctx, sock)) !== 0) {
    ctx.log.die('stack failed to come up — fix the errors above, then re-run `talaria service install`')
  }

  // A unit's ExecStart needs docker's absolute path — systemd's PATH is not
  // the shell's. Whitespace would split into argv; refuse instead of guess.
  const dockerBin = resolveBin('docker', ctx.env)
  if (dockerBin === null) ctx.log.die(`docker not on PATH — the unit needs its absolute path (PATH=${ctx.env.PATH ?? 'unset'})`)
  if (/\s/.test(dockerBin)) ctx.log.die(`${dockerBin} contains whitespace — systemd's ExecStart splits on it; move docker to a path without spaces`)

  const text = unitText({ root: ctx.root, dockerBin, upArgs })
  if (existsSync(unitPath(host))) ctx.log.say('a talaria.service already exists — overwriting (re-install)')
  ctx.log.raw(text)

  // Stage the unit in a temp dir, then hand it to /etc via privileged steps.
  const dir = mkdtempSync(join(tmpdir(), 'talaria-unit-'))
  const staged = join(dir, UNIT_NAME)
  writeFileSync(staged, text)
  if (euid !== 0 && resolveBin('sudo', ctx.env) === null) {
    ctx.log.die(
      `sudo not found and not root — the unit is staged at ${staged} (left in place); run these yourself:\n` +
        `  install -m 0644 ${staged} ${unitPath(host)}\n` +
        '  systemctl daemon-reload\n' +
        `  systemctl enable --now ${UNIT_NAME}`,
    )
  }
  if (euid !== 0 && !ctx.isTTY) ctx.log.warn('no tty — sudo may not be able to ask for a password')
  try {
    const steps: [string, string[], string][] = [
      ['install', ['-m', '0644', staged, unitPath(host)], ''],
      ['systemctl', ['daemon-reload'], ''],
      ['systemctl', ['enable', '--now', UNIT_NAME], 'unit installed but failed to start — `systemctl status talaria` / `journalctl -u talaria`'],
    ]
    for (const [cmd, args, onFail] of steps) {
      const [c, a] = privileged(euid, cmd, args)
      ctx.log.say([c, ...a].join(' '))
      if ((await ctx.run(c, a)) !== 0) ctx.log.die(onFail || `\`${[c, ...a].join(' ')}\` failed`)
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }

  ctx.log.raw(
    [
      'Talaria is supervised. systemd starts the stack at boot and stops it cleanly at shutdown.',
      '',
      '  Status:      systemctl status talaria        (or: bun talaria service status)',
      '  Logs:        journalctl -u talaria -f',
      '  Stop/start:  sudo systemctl stop talaria | sudo systemctl start talaria',
      '  Updates:     bun talaria deploy update       (rebuilds; the unit starts what exists)',
      '  Remove it:   bun talaria service uninstall',
      '',
    ].join('\n'),
  )
  return 0
}

export const installCommand: Leaf = {
  kind: 'leaf',
  name: 'install',
  summary: 'start the stack + install the systemd unit that keeps it running across reboots',
  usage: 'talaria service install',
  run: (ctx) => runInstall(ctx),
}
