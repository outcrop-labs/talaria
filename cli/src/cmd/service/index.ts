// `talaria service` — keep the production compose stack running across
// reboots. install writes a systemd unit around the same docker compose
// project `talaria deploy` drives and enables it; uninstall removes both;
// status shows both sides. docs/CONTAINER.md → "Keep it running across
// reboots".

import type { Ctx } from '../../ctx'
import type { Group, Leaf, ParsedArgs } from '../../cli'
import { envFileText, envValue } from '../../envfile'
import { refuseForeignCheckout } from '../../stack'
import { refuseDeployTargets, warnEnvDrift } from '../deploy'
import { installCommand } from './install'
import { statusCommand } from './status'
import { uninstallCommand } from './uninstall'

/** Every service leaf gets the target guards and the drift warning first,
 *  like deploy's — and install is the one command that FIXES the drift it
 *  names (it pins DOCKER_GID into docker/.env for the boot unit). */
const prelude = (leaf: Leaf): Leaf => ({
  ...leaf,
  run: (ctx: Ctx, args: ParsedArgs) => {
    refuseForeignCheckout(ctx, `service ${leaf.name}`)
    refuseWorktreeUnit(ctx, leaf.name)
    refuseDeployTargets(ctx, `service ${leaf.name}`)
    warnEnvDrift(ctx)
    return leaf.run(ctx, args)
  },
})

/** talaria.service is the host's ONE unit, and it supervises the checkout it
 *  was installed from. A worktree is removed when its pull request merges, so
 *  a unit pointing at one fails at the next boot, and an uninstall from one
 *  stops a stack that was never this worktree's. Status only reads, so it is
 *  left to the project rule above. */
export function refuseWorktreeUnit(ctx: Ctx, leaf: string): void {
  if (leaf !== 'install' && leaf !== 'uninstall') return
  const worktree = envValue(envFileText(ctx, 'ui/.env'), 'TALARIA_WORKTREE')
  if (!worktree) return
  ctx.log.die(
    `this is worktree "${worktree}": \`talaria service ${leaf}\` manages the host's one talaria.service, which is not this worktree's. ` +
      'Run it from the checkout that runs production.',
  )
}

export const serviceCommand: Group = {
  kind: 'group',
  name: 'service',
  summary: 'keep the compose stack running across reboots — a systemd unit (docs/CONTAINER.md)',
  children: [installCommand, uninstallCommand, statusCommand].map(prelude),
}
