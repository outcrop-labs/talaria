// `talaria service uninstall` — remove the unit and stop the stack it
// supervised. disable --now runs the unit's ExecStop (compose down), which
// is the point: plain `disable` would leave the containers running, and
// their restart: unless-stopped would bring them back at the next boot
// AFTER the supervisor is gone — the exact opposite of what was asked.
// Data survives: named volumes and the state dir are untouched (the same
// blast radius as `talaria deploy down`).

import { existsSync } from 'node:fs'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { HOST, UNIT_NAME, type HostPaths, privileged, resolveBin, unitPath } from './shared'

export type UninstallOpts = { host?: HostPaths; euid?: number }

export async function runUninstall(ctx: Ctx, o: UninstallOpts = {}): Promise<number> {
  const host = o.host ?? HOST
  const euid = o.euid ?? process.getuid?.() ?? 1000
  const unit = unitPath(host)

  if (!existsSync(unit)) {
    ctx.log.die(`talaria.service is not installed — nothing to remove (\`talaria service install\`)`)
  }
  if (euid !== 0 && resolveBin('sudo', ctx.env) === null) {
    ctx.log.die(
      `sudo not found and not root — run these yourself:\n` +
        `  systemctl disable --now ${UNIT_NAME}\n` +
        `  rm -f ${unit}\n` +
        '  systemctl daemon-reload\n' +
        `  systemctl reset-failed ${UNIT_NAME}`,
    )
  }

  ctx.log.say('stopping the stack (the unit\'s ExecStop = compose down) — containers are removed; named volumes and the state dir are untouched')
  const steps: [string, string[]][] = [
    ['systemctl', ['disable', '--now', UNIT_NAME]],
    ['rm', ['-f', unit]],
    ['systemctl', ['daemon-reload']],
    ['systemctl', ['reset-failed', UNIT_NAME]],
  ]
  for (const [cmd, args] of steps) {
    const [c, a] = privileged(euid, cmd, args)
    ctx.log.say([c, ...a].join(' '))
    // Tolerated: the unit may already be inactive or unloaded — the goal is
    // the removal, and every failure mode still ends in the file being gone.
    if ((await ctx.run(c, a)) !== 0) ctx.log.warn(`\`${[c, ...a].join(' ')}\` failed — continuing (the removal is what matters)`)
  }

  ctx.log.ok('talaria.service removed — the stack is down and will not come back at boot')
  ctx.log.say('data kept: the pg-data/qdrant-data/minio-data volumes and $TALARIA_STATE_DIR — bring the stack back with `bun talaria deploy up`')
  return 0
}

export const uninstallCommand: Leaf = {
  kind: 'leaf',
  name: 'uninstall',
  summary: 'stop the stack and remove the unit (volumes and state dir are kept)',
  usage: 'talaria service uninstall',
  run: (ctx) => runUninstall(ctx),
}
