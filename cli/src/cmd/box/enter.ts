// `talaria box enter` — a shell (or any command) inside the box, at
// /work/talaria. Port of scripts/devbox `enter`.

import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { requireBox } from './shared'

export function runEnter(ctx: Ctx, name: string, cmd: string[]): Promise<number> {
  requireBox(ctx, name)
  // -it only when there IS a tty: agents and scripts call enter without one,
  // and docker exec refuses "cannot attach stdin to a TTY-enabled container".
  const t = ctx.isTTY ? ['-it'] : []
  return ctx.run('docker', ['exec', ...t, '-w', '/work/talaria', `devbox-${name}`, ...(cmd.length > 0 ? cmd : ['bash'])])
}

export const enterCommand: Leaf = {
  kind: 'leaf',
  name: 'enter',
  summary: 'run a shell (or command) inside a devbox',
  usage: 'talaria box enter <name> [cmd…]',
  positionals: { name: 'name', required: true, multiple: true, desc: 'then any args pass to the command (default: bash)' },
  run: (ctx, args) => runEnter(ctx, args.positionals[0]!, args.positionals.slice(1)),
}
