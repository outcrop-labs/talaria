// `talaria deploy` — the production compose group. Plain compose stays the
// canonical operator path (docs/CONTAINER.md); these wrappers run exactly
// the documented commands and print them first. The prelude below gives
// every subcommand the env-drift warning before anything executes.

import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Group, Leaf, ParsedArgs } from '../../cli'
import { parseEnv } from '../../envfile'
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
  const composeYml = join(ctx.root, 'docker/compose.yml')
  if (!existsSync(composeYml)) return
  // COMPOSE_PROJECT_NAME is honoured without appearing as ${…} in the file.
  const interpolated = new Set(['COMPOSE_PROJECT_NAME'])
  for (const m of readFileSync(composeYml, 'utf8').matchAll(/\$\{([A-Za-z_][A-Za-z0-9_]*)/g)) {
    interpolated.add(m[1]!)
  }
  const envFile = join(ctx.root, 'docker/.env')
  const fileKeys = new Set(existsSync(envFile) ? Object.keys(parseEnv(readFileSync(envFile, 'utf8'))) : [])
  const drifted = [...interpolated]
    .filter((v) => ctx.env[v] !== undefined && ctx.env[v] !== '' && !fileKeys.has(v))
    .sort()
  if (drifted.length === 0) return
  ctx.log.warn(
    `exported in this shell but not in docker/.env: ${drifted.join(', ')}\n` +
      '  Interpolation happens on every up — a later `docker compose -f docker/compose.yml up -d` from a\n' +
      '  shell without them re-interpolates the defaults on a running instance. Move them into\n' +
      "  docker/.env (compose loads it automatically; `talaria deploy` prints what it runs), or export\n" +
      '  them every single time.',
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
