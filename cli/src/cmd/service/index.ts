// `talaria service` — keep the production compose stack running across
// reboots. install writes a systemd unit around the same docker compose
// project `talaria deploy` drives and enables it; uninstall removes both;
// status shows both sides. docs/CONTAINER.md → "Keep it running across
// reboots".

import type { Ctx } from '../../ctx'
import type { Group, Leaf, ParsedArgs } from '../../cli'
import { warnEnvDrift } from '../deploy'
import { installCommand } from './install'
import { statusCommand } from './status'
import { uninstallCommand } from './uninstall'

/** Every service leaf gets the drift warning first, like deploy's — and
 *  install is the one command that FIXES the drift it names (it pins
 *  DOCKER_GID into docker/.env for the boot unit). */
const prelude = (leaf: Leaf): Leaf => ({
  ...leaf,
  run: (ctx: Ctx, args: ParsedArgs) => {
    warnEnvDrift(ctx)
    return leaf.run(ctx, args)
  },
})

export const serviceCommand: Group = {
  kind: 'group',
  name: 'service',
  summary: 'keep the compose stack running across reboots — a systemd unit (docs/CONTAINER.md)',
  children: [installCommand, uninstallCommand, statusCommand].map(prelude),
}
