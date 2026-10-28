// `talaria deploy` — the production compose group. Plain compose stays the
// canonical operator path (docs/CONTAINER.md); these wrappers run exactly
// the documented commands and print them first. The prelude below gives
// every subcommand the env-drift warning before anything executes.

import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Group, Leaf, ParsedArgs } from '../../cli'
import { COMPOSE_BASE, SIDECARS_COMPOSE } from '../../compose'
import { envFileText, envValue, readEnvFile } from '../../envfile'
import { APP_PORT } from '../../ports'
import { refuseForeignCheckout } from '../../stack'
import {
  credsCommand, DEFAULT_DEPLOY_PROJECT, deployProject, downCommand, logsCommand, statusCommand, UPDATE_RESUMED_KEY, updateCommand,
  upCommand,
} from './actions'

/** CONTAINER.md's multi-instance keys, with the compose file's own
 *  defaults: a second instance on one host needs its own value for every one
 *  of them, or the two share a project, a state dir, a port, or a fleet
 *  project and network and "reconcile each other's agents into oblivion". */
export const INSTANCE_KEYS: readonly [key: string, fallback: string][] = [
  ['COMPOSE_PROJECT_NAME', DEFAULT_DEPLOY_PROJECT],
  ['TALARIA_STATE_DIR', '/var/lib/talaria'],
  ['TALARIA_HTTP_PORT', APP_PORT],
  ['TALARIA_FLEET_PROJECT', 'talaria-fleet'],
  ['TALARIA_FLEET_NETWORK', 'talaria'],
]

/** The leaves that create, stop or replace containers, volumes or networks.
 *  logs, status and creds only read. */
const MUTATING = new Set(['up', 'down', 'update'])

/** A worktree owns no production stack. The deploy compose project defaults
 *  to the file's own `talaria`, which on a host that runs one is the host's
 *  production stack, so `deploy up`/`down` in a worktree would rebuild or stop
 *  it from this checkout's code. In a worktree the project must be named
 *  explicitly (COMPOSE_PROJECT_NAME, in the shell or docker/.env), and not as
 *  that default. A leaf that changes anything (`up`, `down`, `update`) needs
 *  more: every one of CONTAINER.md's multi-instance keys, since a project of
 *  its own still binds the production state dir and renders into the
 *  production fleet project (on a dev host, main's dev fleet too) unless each
 *  is its own. No `talaria-wt-*` name either: that prefix is a worktree's dev
 *  stack, and the cleanup sweep removes such a project once no worktree of
 *  that name is left. The same rule covers `service`, which wraps the same
 *  project. The shim case comes first: run from another checkout, every
 *  target here would be that checkout's. */
export function refuseDeployTargets(ctx: Ctx, command: string, leaf = ''): void {
  refuseForeignCheckout(ctx, command)
  const worktree = envValue(envFileText(ctx, 'ui/.env'), 'TALARIA_WORKTREE')
  if (!worktree) return
  // Compose's own view: the shell over docker/.env, and `${X:-default}`
  // reads an empty value as unset.
  const effective = readEnvFile(ctx, 'docker/.env', { envWins: true, quotes: true })
  const keys = MUTATING.has(leaf) ? INSTANCE_KEYS : INSTANCE_KEYS.slice(0, 1)
  const problems: string[] = []
  for (const [key, fallback] of keys) {
    const value = key === 'COMPOSE_PROJECT_NAME' ? deployProject(ctx) : effective[key] || fallback
    if (value === fallback) {
      problems.push(`    ${key}: '${value}', the compose file's default, which the production instance uses`)
    } else if ((key === 'COMPOSE_PROJECT_NAME' || key === 'TALARIA_FLEET_PROJECT') && value.startsWith('talaria-wt-')) {
      problems.push(`    ${key}: '${value}', a worktree dev stack's name, which the cleanup sweep removes`)
    }
  }
  if (problems.length === 0) return
  const suggest: Record<string, string> = {
    COMPOSE_PROJECT_NAME: `talaria-${worktree}-deploy`,
    TALARIA_STATE_DIR: `/var/lib/talaria-${worktree}`,
    TALARIA_HTTP_PORT: '<a free port>',
    TALARIA_FLEET_PROJECT: `talaria-${worktree}-deploy-fleet`,
    TALARIA_FLEET_NETWORK: `talaria-${worktree}-deploy`,
  }
  ctx.log.die(
    `this is worktree "${worktree}", and \`talaria ${command}\` would act on targets that are not this worktree's own. ` +
      'Refusing rather than guess:\n' +
      `${problems.join('\n')}\n` +
      (MUTATING.has(leaf)
        ? '  A second instance on this host needs its own value for each of these (docs/CONTAINER.md, "more than one\n' +
          "  instance per host\"), plus a matching network.name in that instance's fleet/chassis.yml.\n"
        : '') +
      '  Name them in docker/.env (or export them), for example:\n' +
      keys.map(([k]) => `    ${k}=${suggest[k]}`).join('\n'),
  )
}

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

/** Every deploy leaf gets the target guard and the drift warning first —
 *  declared once here, not six times in actions. Not twice for one update: the half `runUpdate`
 *  re-launches after a sync that moved the checkout has already had it. */
const prelude = (leaf: Leaf): Leaf => ({
  ...leaf,
  run: (ctx: Ctx, args: ParsedArgs) => {
    refuseDeployTargets(ctx, `deploy ${leaf.name}`, leaf.name)
    if (!ctx.env[UPDATE_RESUMED_KEY]) warnEnvDrift(ctx)
    return leaf.run(ctx, args)
  },
})

export const deployCommand: Group = {
  kind: 'group',
  name: 'deploy',
  summary: 'production compose wrappers — up/down/update/logs/creds/status (docs/CONTAINER.md)',
  children: [upCommand, downCommand, updateCommand, logsCommand, credsCommand, statusCommand].map(prelude),
}
