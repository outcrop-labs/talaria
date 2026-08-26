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
