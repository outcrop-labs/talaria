// `talaria seed-target` — fill this checkout's empty Rust build cache from an
// idle checkout's, by reflink: the registry crates arrive built, and every
// workspace crate builds from this checkout's own sources. `talaria worktree`
// runs the same step for a new worktree; this is it on its own, for a
// worktree that predates it, a target removed by `talaria cleanup`, or the
// desktop target before a desktop build. The why: src/targetseed.ts.

import { existsSync, realpathSync } from 'node:fs'
import { resolve } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import { seedCheckout } from '../targetseed'

export async function runSeedTarget(ctx: Ctx, opts: { from?: string; desktop?: boolean }): Promise<number> {
  const from = opts.from ? resolve(ctx.cwd ?? ctx.root, opts.from) : undefined
  if (from && !existsSync(from)) ctx.log.die(`--from ${from}: no such checkout`)
  // The checkout you stand in, which is not always the one whose CLI runs:
  // the installed shim runs the primary's from anywhere.
  let checkout = ctx.root
  try {
    const top = (await ctx.exec('git', ['rev-parse', '--show-toplevel'], { cwd: ctx.cwd ?? ctx.root })).stdout.trim()
    if (top) checkout = realpathSync(top)
  } catch {
    // not in a git checkout: the CLI's own
  }
  ctx.log.say(`Rust build cache for ${checkout}`)
  const results = await seedCheckout(ctx, checkout, { from, desktop: opts.desktop })
  return results.some((r) => r.seeded) ? 0 : 1
}

export const seedTargetCommand: Leaf = {
  kind: 'leaf',
  name: 'seed-target',
  summary: "seed this checkout's empty Rust build cache from an idle checkout's (reflink; registry crates only)",
  usage: 'talaria seed-target [--from <checkout>] [--desktop]',
  flags: [
    { name: 'from', kind: 'value', desc: 'the checkout to seed from (default: the idle one whose Cargo.lock shares the most entries)' },
    { name: 'desktop', kind: 'bool', desc: 'seed desktop/src-tauri/target as well as api/target' },
  ],
  run: (ctx, args) =>
    runSeedTarget(ctx, { from: typeof args.flags.from === 'string' ? args.flags.from : undefined, desktop: args.flags.desktop === true }),
}
