// `talaria service status` — the systemd side (is the unit installed,
// enabled, did its last start succeed), then the compose side via deploy's
// status (effective port/state/fleet + compose ps). `systemctl show` is
// unprivileged, so no sudo here.

import { existsSync } from 'node:fs'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { runStatus } from '../deploy/actions'
import { HOST, UNIT_NAME, type HostPaths, unitPath, unitState } from './shared'

export type StatusOpts = { host?: HostPaths }

export async function runServiceStatus(ctx: Ctx, o: StatusOpts = {}): Promise<number> {
  const host = o.host ?? HOST
  if (!existsSync(unitPath(host))) {
    ctx.log.die(`talaria.service is not installed — \`talaria service install\` (docs/CONTAINER.md)`)
  }

  const s = await unitState(ctx, UNIT_NAME, ['LoadState', 'UnitFileState', 'ActiveState', 'SubState', 'ExecMainStatus'])
  ctx.log.say(`${UNIT_NAME}: ${s.UnitFileState || s.LoadState || 'unknown'}`)
  const failed = s.ExecMainStatus !== undefined && s.ExecMainStatus !== '0' ? `, last exit ${s.ExecMainStatus}` : ''
  ctx.log.say(`state: ${s.ActiveState ?? 'unknown'}${s.SubState ? ` (${s.SubState})` : ''}${failed}`)
  if (s.ActiveState === 'active') ctx.log.skip('active on a oneshot = started successfully (RemainAfterExit)')
  ctx.log.skip('detail: systemctl status talaria · journalctl -u talaria -f')

  return runStatus(ctx)
}

export const statusCommand: Leaf = {
  kind: 'leaf',
  name: 'status',
  summary: 'the unit state (enabled/active), then the compose view',
  usage: 'talaria service status',
  run: (ctx) => runServiceStatus(ctx),
}
