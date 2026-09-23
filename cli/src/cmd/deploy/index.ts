// `talaria deploy` — the production compose group. Plain compose stays the
// canonical operator path (docs/CONTAINER.md); these wrappers run exactly
// the documented commands and print them first. The prelude below gives
// every subcommand the env-drift warning before anything executes.

import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Group, Leaf, ParsedArgs } from '../../cli'
import { COMPOSE_BASE, SIDECARS_COMPOSE } from '../../compose'
import { readEnvFile } from '../../envfile'
import { credsCommand, downCommand, logsCommand, statusCommand, updateCommand, upCommand } from './actions'

/** CONTAINER.md's one habit, enforced: a variable you override on the
 *  command line belongs in docker/.env, because interpolation happens on
 *  EVERY up and a later plain `docker compose up -d` from a shell without
 *  the exports silently re-interpolates the defaults — republishing the
 *  default port and remounting the default state dir on a running
 *  instance. The CLI can see that drift coming: anything the compose file
 *  interpolates (the set is read FROM the file, so new knobs join
 *  automatically) that is exported in this shell but absent from
 *  docker/.env is called out before it bites. File-present is not drift,
 *  and neither is an empty export — `${VAR:-default}` treats empty as
 *  unset. */
export function warnEnvDrift(ctx: Ctx): void {
  // Every compose file the next invocation will actually read: the shared
  // sidecar plane (composeFileArgs layers it first — its ${…}s interpolate
  // too), the base file, plus whatever a COMPOSE_FILE env layers on top (the
  // registry-image flow in CONTAINER.md), whose knobs (TALARIA_CHANNEL, …)
  // join the checked set. Missing files are skipped.
  const listed = [SIDECARS_COMPOSE, COMPOSE_BASE, ...(ctx.env.COMPOSE_FILE ?? '').split(':')]
    .map((p) => p.trim())
    .filter(Boolean)
  const interpolated = new Set(['COMPOSE_PROJECT_NAME'])
  let read = false
  for (const rel of listed) {
    const path = join(ctx.root, rel)
    if (!existsSync(path)) continue
    read = true
    for (const m of readFileSync(path, 'utf8').matchAll(/\$\{([A-Za-z_][A-Za-z0-9_]*)/g)) {
      interpolated.add(m[1]!)
    }
  }
  if (!read) return
  const fileVars = readEnvFile(ctx, 'docker/.env')
  const drifted = [...interpolated]
    .filter((v) => ctx.env[v] !== undefined && ctx.env[v] !== '' && !Object.hasOwn(fileVars, v))
    .sort()
  if (drifted.length === 0) return
  ctx.log.warn(
    `exported in this shell but not in docker/.env: ${drifted.join(', ')}\n` +
      '  Interpolation happens on every up — a later `docker compose -f docker/sidecars.compose.yml -f\n' +
      '  docker/compose.yml up -d` from a shell without them re-interpolates the defaults on a running\n' +
      '  instance. Move them into docker/.env (compose loads it automatically; `talaria deploy` prints\n' +
      '  what it runs), or export them every single time.',
  )
}

/** Every deploy leaf gets the drift warning first — declared once here, not
 *  six times in actions. */
const prelude = (leaf: Leaf): Leaf => ({
  ...leaf,
  run: (ctx: Ctx, args: ParsedArgs) => {
    warnEnvDrift(ctx)
    return leaf.run(ctx, args)
  },
})

export const deployCommand: Group = {
  kind: 'group',
  name: 'deploy',
  summary: 'production compose wrappers — up/down/update/logs/creds/status (docs/CONTAINER.md)',
  children: [upCommand, downCommand, updateCommand, logsCommand, credsCommand, statusCommand].map(prelude),
}
