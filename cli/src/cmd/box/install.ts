// `talaria box install` — put a tool (your harness of choice) into the layer
// EVERY box mounts at /work/tools: install once from any box, use it from all
// of them — including boxes that don't exist yet. The command runs inside the
// named box with npm's global prefix pointed at the layer and flock held
// (toolsExec). docs/DEVBOX.md, "The agent CLIs".

import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { requireBox, toolsExec } from './shared'

export function runInstall(ctx: Ctx, name: string, cmd: string): Promise<number> {
  requireBox(ctx, name)
  if (!cmd.trim()) ctx.log.die('pass the install command, quoted as one arg (e.g. box install demo \'npm i -g @openai/codex\')')
  ctx.log.say(`Shared tools layer, via ${name} (flock-held): ${cmd}`)
  return ctx.run('docker', toolsExec(name, cmd))
}

export const installCommand: Leaf = {
  kind: 'leaf',
  name: 'install',
  summary: 'install a tool/harness into the layer every devbox shares',
  usage: 'talaria box install <name> <cmd…>',
  positionals: { name: 'name', required: true, multiple: true, desc: 'box to run the install in, then the command (quote it as one arg)' },
  run: (ctx, args) => runInstall(ctx, args.positionals[0]!, args.positionals.slice(1).join(' ')),
}
