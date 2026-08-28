// Shared plumbing for the box commands — the devbox registry layout, the
// compose tuple every box operation runs through, and the name gate.
//
// Boxes live in ../devboxes/<name>/ (override: TALARIA_DEVBOX_HOME):
//   talaria/       the clone (branch agent/<name>), at /work/talaria in-box
//   state/         fleet/apps/uploads — bind-mounted at the SAME path (the
//                  fleet renderer bakes absolute host paths into agent binds)
//   compose.env    per-box interpolation (ports, paths, creds) — 0600
//   compose.override.yml  optional: extra env for the box (auth tokens, GLM
//                  provider vars); merged when present
//   box.env        the registry `ls` reads
// Sibling: ../devboxes/shared/tools/ — the tools layer EVERY box mounts at
// /work/tools (harness of choice installed once, used from all of them).

import { existsSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { ComposeSpec } from '../../compose'
import { devboxHome, NAME_RE } from '../../paths'

export const IMAGE = 'talaria-devbox:latest'
export const COMPOSE_FILE = 'docker/devbox.compose.yml'

export const devboxes = (ctx: Ctx): string => devboxHome(ctx.root, ctx.env)

export const boxDir = (ctx: Ctx, name: string): string => join(devboxes(ctx), name)

export const boxState = (ctx: Ctx, name: string): string => join(boxDir(ctx, name), 'state')

/** The shared tools layer's host dir (mounted at /work/tools in every box —
 *  docker/devbox.compose.yml). The alternative, plain `npm i -g` inside a
 *  box, fails twice: the image's global prefix is root-owned (EACCES as
 *  dev), and anything in the container's writable layer dies with it. */
export const sharedTools = (ctx: Ctx): string => join(devboxes(ctx), 'shared', 'tools')

/** The docker argv that runs a command inside a box AGAINST the shared tools
 *  layer: npm's global prefix pointed at it, TOOLS_* for any other installer,
 *  flock-held so two boxes can't interleave installs into the same dir. The
 *  user command rides as `$0` — passed as argv, never re-quoted. Pure. */
export function toolsExec(name: string, cmd: string): string[] {
  return [
    'exec',
    '-e', 'NPM_CONFIG_PREFIX=/work/tools',
    '-e', 'TOOLS_DIR=/work/tools',
    '-e', 'TOOLS_BIN=/work/tools/bin',
    `devbox-${name}`,
    'sh', '-lc', 'exec flock /work/tools/.lock -c "$0"',
    cmd,
  ]
}

/** The repeated tuple: this template + this box's interpolation + its
 *  project. compose.override.yml (auth/provider env) merges in when the box
 *  has one. */
export function boxComposeSpec(ctx: Ctx, name: string): ComposeSpec {
  const files = [join(ctx.root, COMPOSE_FILE)]
  const override = join(boxDir(ctx, name), 'compose.override.yml')
  if (existsSync(override)) files.push(override)
  return { files, project: `devbox-${name}`, envFile: join(boxDir(ctx, name), 'compose.env') }
}

export function requireBox(ctx: Ctx, name: string): void {
  if (!existsSync(join(boxDir(ctx, name), 'box.env'))) {
    ctx.log.die(`no devbox named '${name}' (see: bun talaria box ls)`)
  }
}

/** Validate a box name — lowercase kebab; it lands in container names,
 *  networks and directories. */
export function checkName(ctx: Ctx, name: string): void {
  if (!NAME_RE.test(name)) ctx.log.die('name must be lowercase-kebab (a-z0-9-)')
}
