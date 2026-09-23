// `talaria cleanup` — flag or sweep the artifacts a dev task leaves behind.
// The policy lives in scripts/cleanup-sweep.mjs, which the stop gate runs
// itself; this command is the same script, so `talaria cleanup` and the gate
// cannot drift.

import { join } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'

export function runCleanup(ctx: Ctx, apply: boolean): Promise<number> {
  const script = join(ctx.root, 'scripts/cleanup-sweep.mjs')
  return ctx.run(process.execPath, [script, ...(apply ? ['--apply'] : [])], { cwd: ctx.root })
}

export const cleanupCommand: Leaf = {
  kind: 'leaf',
  name: 'cleanup',
  summary: 'flag or sweep stale dev artifacts and report disk pressure',
  usage: 'talaria cleanup [--apply]',
  flags: [
    {
      name: 'apply',
      kind: 'bool',
      desc: 'remove the safe set (stale clean worktrees and boxes, old /tmp/talaria-*, orphan compose projects)',
    },
  ],
  run: (ctx, args) => runCleanup(ctx, args.flags.apply === true),
}