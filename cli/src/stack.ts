// The dev stack a command acts on, resolved from the checkout's own ui/.env,
// and the refusal when it cannot be named without guessing.
//
// WHY THIS EXISTS. `talaria reset database|secrets|fleet`, run inside a
// worktree, acted on the MAIN dev stack. It exec'd into
// `TALARIA_PG_CONTAINER ?? 'talaria-postgres-dev'`: nothing sets that env var
// in a worktree, so it took the default, which is main's container. Its fleet
// sweep also picked containers by the name prefix `talaria-fleet-agent`, which
// only main's fleet project produces. The worktree's ui/.env said exactly
// which stack it was, and nothing read it.
//
// THE RULE (docs/WORKTREES.md). Inside a worktree (its ui/.env carries the
// TALARIA_WORKTREE marker that `talaria worktree` writes), every container,
// compose project and database a command acts on comes from that worktree.
// Containers are picked by their compose project LABEL, never by a name
// prefix. When the answer is ambiguous, the command refuses before it touches
// anything and says why:
//
//   · the shell names a container, project or worktree that is not this
//     stack's;
//   · a linked git worktree has no marker, so its DATABASE_URL may be main's;
//   · the operator stands in one checkout and runs another checkout's CLI.
//     The installed `talaria` shim always runs the primary's CLI, whose root is
//     the primary, so inside a worktree it would act on the primary's stack;
//   · the postgres container the command would exec into is not the one the
//     checkout's DATABASE_URL names: another compose project, another port,
//     or a database and role other than the `talaria` ones it execs as;
//   · in a worktree, the shell overrides a line its ui/.env owns
//     (DATABASE_URL, REDIS_URL, a port, the fleet project) with another
//     stack's value. `talaria dev` lifts the shell over ui/.env, so a stale
//     `DATABASE_URL=…:5544` put a second server on main's database;
//   · a command that reads or starts MAIN's containers by name (the seed
//     source of `talaria box new`/`box seed`, the primary's `talaria dev`)
//     finds a TALARIA_*_CONTAINER in the shell that names a worktree's or a
//     box's container. A worktree's `talaria dev` sets exactly those
//     variables in its own process, so every process it starts inherits them
//     (primaryContainer, refuseSideStackContainers).
//
// The names below mirror what cmd/worktree.ts creates for a worktree. Change
// them together.

import { existsSync, realpathSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import type { Ctx } from './ctx'
import { EMBED_CONTAINER, PG_CONTAINER, QDRANT_CONTAINER, REDIS_CONTAINER, STORAGE_CONTAINER } from './containers'
import { envFileText, envValue } from './envfile'

/** docker/dev-compose.yml's `name:`, the project that owns the primary's
 *  postgres and redis. */
export const PRIMARY_INFRA_PROJECT = 'talaria-dev'

/** talaria-fleet-layout's default fleet project, used when nothing names one. */
export const PRIMARY_FLEET_PROJECT = 'talaria-fleet'

/** The compose label docker stamps on every container a project created. */
export const COMPOSE_PROJECT_LABEL = 'com.docker.compose.project'

/** Every docker name a `talaria worktree <name>` stack owns. */
export function worktreeStackNames(name: string): { pg: string; redis: string; infra: string; fleet: string } {
  return {
    pg: `talaria-pg-${name}`,
    redis: `talaria-redis-${name}`,
    infra: `talaria-wt-${name}`,
    fleet: `talaria-wt-${name}-fleet`,
  }
}

export type DevStack = {
  kind: 'primary' | 'worktree'
  /** TALARIA_WORKTREE; null for the primary. */
  worktree: string | null
  pgContainer: string
  redisContainer: string
  /** The compose project that owns postgres and redis. */
  infraProject: string
  /** The fleet's compose project, or null when it cannot be named without
   *  guessing. fleetProblem then says why. */
  fleetProject: string | null
  fleetProblem: string | null
}

/** The Talaria checkout that contains `dir`: the nearest directory at or above
 *  it with both a `.git` and the CLI's entry point. A `.git` without the CLI
 *  (an app's own repo under apps/, say) does not stop the walk, because the
 *  checkout it sits in is still the one the operator is standing in. Null
 *  when `dir` is in no Talaria checkout. */
export function checkoutAt(dir: string): string | null {
  let d: string
  try {
    d = realpathSync(dir)
  } catch {
    return null
  }
  for (let i = 0; i < 64; i++) {
    if (existsSync(join(d, '.git')) && existsSync(join(d, 'cli/bin/talaria.ts'))) return d
    const parent = dirname(d)
    if (parent === d) return null
    d = parent
  }
  return null
}

/** Refuse when the operator stands in a different Talaria checkout from the
 *  one this CLI belongs to. The installed `talaria` shim always runs the
 *  primary's CLI, so inside a worktree its ctx.root is the primary, and
 *  `talaria reset` there resets the primary. Which stack is meant is
 *  ambiguous, so the command stops and names the command that is not. */
export function refuseForeignCheckout(ctx: Ctx, command: string): void {
  if (!ctx.cwd) return
  const standing = checkoutAt(ctx.cwd)
  if (!standing || standing === ctx.root) return
  ctx.log.die(
    `this \`talaria\` runs the CLI of the checkout at ${ctx.root}, but you are standing in ${standing}.\n` +
      `  \`talaria ${command}\` would act on ${ctx.root}'s stack, not the one you are in, so it refuses rather than guess.\n` +
      `  For the checkout you are in:  cd ${standing} && bun talaria ${command}\n` +
      `  For ${ctx.root}'s own stack:  run it from ${ctx.root}`,
  )
}

const isFile = (p: string): boolean => {
  try {
    return statSync(p).isFile()
  } catch {
    return false
  }
}

/** The stack this checkout owns. It dies on every ambiguity the module header
 *  lists that can be settled from config alone. The docker-side check,
 *  whether the container IS the one DATABASE_URL names, is verifyPostgres.
 *  `uiEnv` is ui/.env's text when the caller has already read it; `command`
 *  is what a refusal tells the operator to re-run. */
export function devStack(ctx: Ctx, uiEnv: string = envFileText(ctx, 'ui/.env'), command?: string): DevStack {
  const box = ctx.env.TALARIA_DEVBOX || envValue(uiEnv, 'TALARIA_DEVBOX')
  if (box) {
    ctx.log.die(
      `this checkout is devbox "${box}": the box compose owns its postgres and redis, and this command acts on a host dev stack. ` +
        'Run it on the host, in the checkout whose stack you mean.',
    )
  }
  const worktree = envValue(uiEnv, 'TALARIA_WORKTREE') || null
  const shellWorktree = ctx.env.TALARIA_WORKTREE
  if (shellWorktree && shellWorktree !== (worktree ?? '')) {
    // Most often a shell under that worktree's `talaria dev`: it inherited
    // the worktree's whole stack, not only the marker (WORKTREE_INHERITED_VARS),
    // and in the primary the shell wins over ui/.env. So the way out unsets
    // all of it; `unset TALARIA_WORKTREE` alone would pass this check and run
    // on the worktree's DATABASE_URL, fleet project and ports.
    ctx.log.die(
      `TALARIA_WORKTREE=${shellWorktree} in the environment, but ${join(ctx.root, 'ui/.env')} ` +
        (worktree ? `marks worktree "${worktree}"` : 'carries no worktree marker') +
        ". A shell under a worktree's `talaria dev` inherits that worktree's stack lines with it " +
        '(its DATABASE_URL, fleet project and ports among them), so this refuses rather than guess which stack you meant.\n' +
        `  Run it without them: ${inheritedUnsetCommand(ctx, ['TALARIA_WORKTREE'], command)}\n` +
        '  or run it from that worktree.',
    )
  }

  if (!worktree) {
    // A linked git worktree (its .git is a file) without the marker was made
    // by hand. Its DATABASE_URL may well be main's, and its fleet would be.
    if (isFile(join(ctx.root, '.git'))) {
      ctx.log.die(
        `${ctx.root} is a git worktree without an isolated stack (no TALARIA_WORKTREE in ui/.env), ` +
          'so which database and fleet it owns is a guess. Create isolated worktrees with ' +
          '`bun talaria worktree <name>` (docs/WORKTREES.md).',
      )
    }
    // The api's own resolution order: the shell, then ui/.env (dev.ts lifts
    // it), then the layout's default. A `talaria-wt-*` name is a worktree's
    // fleet, never the primary's, the same rule verifyPostgres applies to the
    // primary's postgres: a stale shell var must not point the primary's
    // `reset fleet` at a worktree's agents.
    const fleet = ctx.env.TALARIA_FLEET_PROJECT ?? envValue(uiEnv, 'TALARIA_FLEET_PROJECT') ?? PRIMARY_FLEET_PROJECT
    const theirs = fleet.startsWith('talaria-wt-')
    // The same rule for the containers, by name: a shell override shaped like
    // a worktree's or a box's container is that stack's, never the primary's.
    // verifyPostgres adds the label check once docker is asked.
    const byName = sideStackContainers(ctx, ['TALARIA_PG_CONTAINER', 'TALARIA_REDIS_CONTAINER'])
    if (byName.length > 0) ctx.log.die(sideStackRefusal(ctx, byName, command))
    return {
      kind: 'primary',
      worktree: null,
      pgContainer: primaryContainer(ctx, 'TALARIA_PG_CONTAINER'),
      redisContainer: primaryContainer(ctx, 'TALARIA_REDIS_CONTAINER'),
      infraProject: PRIMARY_INFRA_PROJECT,
      fleetProject: theirs ? null : fleet,
      fleetProblem: theirs
        ? `TALARIA_FLEET_PROJECT=${fleet} is a worktree's fleet project, and this is the primary checkout. ` +
          "Refusing rather than act on that worktree's agents: unset it, or run from that worktree."
        : null,
    }
  }

  const own = worktreeStackNames(worktree)
  for (const [key, mine] of [
    ['TALARIA_PG_CONTAINER', own.pg],
    ['TALARIA_REDIS_CONTAINER', own.redis],
  ] as const) {
    const named = ctx.env[key]
    if (named !== undefined && named !== mine) {
      ctx.log.die(
        `this is worktree "${worktree}", whose container is ${mine}, but ${key}=${named} in the environment names another. ` +
          `Refusing rather than guess which one you meant: unset ${key}.`,
      )
    }
  }

  // The fleet project comes from where the api reads it: the shell, then
  // ui/.env. A worktree cut before `talaria worktree` wrote the line rendered
  // into the default project, which is main's, so its agents cannot be told
  // from main's by project.
  const named = ctx.env.TALARIA_FLEET_PROJECT ?? envValue(uiEnv, 'TALARIA_FLEET_PROJECT')
  let fleetProject: string | null = own.fleet
  let fleetProblem: string | null = null
  if (named === undefined || named === '') {
    fleetProject = null
    fleetProblem =
      `worktree "${worktree}" names no TALARIA_FLEET_PROJECT in ui/.env, so its api renders into the default project ` +
      `${PRIMARY_FLEET_PROJECT}, which is main's, and its agents cannot be told from main's. ` +
      `Add TALARIA_FLEET_PROJECT=${own.fleet} to ui/.env and re-render, or recut the worktree.`
  } else if (named !== own.fleet) {
    fleetProject = null
    fleetProblem =
      `TALARIA_FLEET_PROJECT=${named} is not worktree "${worktree}"'s fleet project (${own.fleet}). ` +
      'Refusing rather than act on another stack\'s agents.'
  }
  return {
    kind: 'worktree',
    worktree,
    pgContainer: own.pg,
    redisContainer: own.redis,
    infraProject: own.infra,
    fleetProject,
    fleetProblem,
  }
}

/** The lines `talaria worktree` writes into a worktree's ui/.env that name
 *  its stack: its database, its redis, its ports, the url its proxy dials and
 *  its fleet project (cmd/worktree.ts; keep the two lists together). */
export const WORKTREE_OWN_VARS = [
  'DATABASE_URL',
  'REDIS_URL',
  'PORT',
  'TALARIA_API_PORT',
  'TALARIA_RUST_API_URL',
  'TALARIA_GATEWAY_SELF_URL',
  'TALARIA_FLEET_PROJECT',
  'TALARIA_MCP_PORT',
  'TALARIA_OMP_AUTH_PORT',
  'TALARIA_GATEWAY_PORT_BASE',
] as const

/** A value safe to print: a url's userinfo (`user:password@`) cut, anything
 *  else as it is. */
export const printable = (v: string): string => v.replace(/^([a-z][a-z0-9+.-]*:\/\/)[^/]*@/i, '$1')

/** Refuse when, in a worktree, the shell sets a line its ui/.env owns to
 *  another value. Both name a target and they disagree: the shell value is
 *  most often a stale export from main's or another worktree's shell, and
 *  `talaria dev` lets the shell win (apiEnv, and vite's own loader). A
 *  `DATABASE_URL` naming main's port would make this worktree's api a second
 *  server on main's database. A line ui/.env does not carry is left to the
 *  shell, as before; a shell value equal to ui/.env's is not a conflict. The
 *  primary keeps the shell as its override point. */
export function refuseWorktreeEnvOverride(ctx: Ctx, stack: DevStack, uiEnv: string, command: string): void {
  if (stack.kind !== 'worktree') return
  const keys = WORKTREE_OWN_VARS.filter((key) => {
    const shell = ctx.env[key]
    const own = envValue(uiEnv, key)
    return shell !== undefined && own !== undefined && shell !== own
  })
  if (keys.length === 0) return
  const lines = keys.map((k) => `    ${k}=${printable(ctx.env[k]!)}   (ui/.env: ${printable(envValue(uiEnv, k)!)})`)
  ctx.log.die(
    `this is worktree "${stack.worktree}", and the environment overrides lines its ui/.env owns:\n` +
      `${lines.join('\n')}\n` +
      `  \`talaria ${command}\` lets the shell win, so this stack would run against another stack's targets ` +
      "(a DATABASE_URL of main's makes a second server on main's database). Refusing rather than guess.\n" +
      `  Unset them (unset ${keys.join(' ')}), or run from the checkout they belong to.`,
  )
}

// ── Main's containers, named by the shell ───────────────────────────────────
//
// dev-compose.yml names the primary's containers `${TALARIA_*_CONTAINER:-…}`,
// and the shell is the primary's override point for them. Some commands read
// or start MAIN's containers on purpose: `talaria box new` and `box seed` copy
// main's postgres (and storage, and qdrant) into a box, and the primary's
// `talaria dev` brings main's infra up. They read those variables from the
// shell. But a worktree's `talaria dev` sets TALARIA_PG_CONTAINER and
// TALARIA_REDIS_CONTAINER (and the two ports) to its own, in its own process,
// so the app, its harness runs and every shell they open inherit them. From
// such a shell, `box new` seeded the box from that worktree and called it
// main. Worse, the primary compose `up` that `box new` and the primary's
// `talaria dev` run would interpolate the worktree's name into MAIN's
// project: compose recreates main's postgres under that name (and the
// worktree's port), or, while the worktree's own container holds the name,
// removes main's and fails on the name conflict, leaving main down. An
// override that names another stack's container is refused, before anything
// is created.

/** dev-compose.yml's container-name variables, each with the primary's
 *  default (containers.ts). TALARIA_MINIO_CONTAINER is the MinIO-era
 *  spelling of the storage container, still read as its fallback. */
export const PRIMARY_CONTAINER_VARS = {
  TALARIA_PG_CONTAINER: PG_CONTAINER,
  TALARIA_REDIS_CONTAINER: REDIS_CONTAINER,
  TALARIA_QDRANT_CONTAINER: QDRANT_CONTAINER,
  TALARIA_EMBED_CONTAINER: EMBED_CONTAINER,
  TALARIA_STORAGE_CONTAINER: STORAGE_CONTAINER,
  TALARIA_MINIO_CONTAINER: STORAGE_CONTAINER,
} as const
export type PrimaryContainerVar = keyof typeof PRIMARY_CONTAINER_VARS
export const ALL_PRIMARY_CONTAINER_VARS = Object.keys(PRIMARY_CONTAINER_VARS) as PrimaryContainerVar[]

/** What a worktree's `talaria dev` sets for its own compose call, and so what
 *  a shell started under it inherits (cmd/dev.ts, the worktree branch). */
export const WORKTREE_DEV_EXPORTS = ['TALARIA_PG_CONTAINER', 'TALARIA_PG_PORT', 'TALARIA_REDIS_CONTAINER', 'TALARIA_REDIS_PORT'] as const

/** Everything a shell under a worktree's `talaria dev` inherits that names
 *  that worktree's stack: the exports above, which dev sets in its own
 *  process, and the lines `talaria worktree` writes into its ui/.env
 *  (TALARIA_WORKTREE and WORKTREE_OWN_VARS, worktree.ts WORKTREE_ENV_KEYS).
 *  vite loads that file into its process.env (ui/vite.config.ts apiDev) and
 *  apiEnv lifts the same lines into the api's, so every process the app or
 *  the api starts carries them. */
export const WORKTREE_INHERITED_VARS = [...WORKTREE_DEV_EXPORTS, 'TALARIA_WORKTREE', ...WORKTREE_OWN_VARS] as const

/** The command a refusal of an inherited worktree shell prints: `env -u` for
 *  `keys`, then for every WORKTREE_INHERITED_VARS variable the shell sets,
 *  then `bun talaria <command>`. Unsetting one this checkout's ui/.env carries
 *  costs nothing (apiEnv and vite fall back to the file), and one it does not
 *  carry is an inherited line that would otherwise win: the primary keeps the
 *  shell as its override point, so a worktree's DATABASE_URL, fleet project
 *  or api url there would put main's servers on the worktree's stack. */
export function inheritedUnsetCommand(ctx: Ctx, keys: readonly string[], command?: string): string {
  const all = [...new Set([...keys, ...WORKTREE_INHERITED_VARS.filter((k) => ctx.env[k] !== undefined)])]
  return `env ${all.map((k) => `-u ${k}`).join(' ')} bun talaria ${command ?? '<command>'}`
}

/** A compose project that is another stack's, never main's: a worktree's
 *  (`talaria-wt-*`, its infra and its fleet) or a devbox's (`devbox-*`). */
export const isSideProject = (project: string): boolean => project.startsWith('talaria-wt-') || project.startsWith('devbox-')

/** Whose container a name is by its shape alone: a worktree's
 *  (`talaria-pg-<name>`, `talaria-redis-<name>`, worktreeStackNames, or
 *  anything of a `talaria-wt-*` project) or a devbox's (`devbox-*`, shared.ts).
 *  Null for any other name. The caller skips the primary's own defaults:
 *  `talaria-redis-dev` has a worktree's shape. */
export function sideStackOfName(name: string): string | null {
  const m = /^talaria-(pg|redis)-([a-z0-9][a-z0-9-]*)$/.exec(name)
  if (m) return `worktree "${m[2]}"'s ${m[1] === 'pg' ? 'postgres' : 'redis'}`
  if (name.startsWith('talaria-wt-')) return "a worktree's stack (talaria-wt-*)"
  if (name.startsWith('devbox-')) return "a devbox's stack (devbox-*)"
  return null
}

/** Main's container for `key`: the shell's value, or the primary's default
 *  when the shell sets none or an empty one (compose's `:-` reads empty as
 *  unset). Storage falls back to the MinIO-era variable. Call it after
 *  refuseSideStackContainers (or sideStackContainers), which is what makes
 *  the shell's value main's. */
export function primaryContainer(ctx: Ctx, key: PrimaryContainerVar): string {
  const legacy = key === 'TALARIA_STORAGE_CONTAINER' ? ctx.env.TALARIA_MINIO_CONTAINER : undefined
  return ctx.env[key] || legacy || PRIMARY_CONTAINER_VARS[key]
}

/** One shell variable that names another stack's container, and why. */
export type SideStackOverride = { key: PrimaryContainerVar; value: string; whose: string }

/** The shell's `keys` whose value has another stack's shape. Pure: no docker
 *  call, so it also catches a name whose container is gone, which compose
 *  would CREATE under main's project with that name. A value equal to the
 *  primary's default is main's by definition. */
export function sideStackContainers(ctx: Ctx, keys: readonly PrimaryContainerVar[]): SideStackOverride[] {
  const out: SideStackOverride[] = []
  for (const key of keys) {
    const value = ctx.env[key]
    if (!value || value === PRIMARY_CONTAINER_VARS[key]) continue
    const whose = sideStackOfName(value)
    if (whose) out.push({ key, value, whose })
  }
  return out
}

/** The compose project docker says a container belongs to: '' for none,
 *  null when docker does not know it (or does not answer). */
export async function containerProject(ctx: Ctx, name: string): Promise<string | null> {
  try {
    const info = (JSON.parse((await ctx.exec('docker', ['inspect', name])).stdout) as Inspect[])[0]
    return info ? (info.Config?.Labels?.[COMPOSE_PROJECT_LABEL] ?? '') : null
  } catch {
    return null
  }
}

/** The refusal for `found`: each variable with what it names, the
 *  inherited-from-a-worktree reading, and the command to run instead. The
 *  `env -u` list carries every WORKTREE_INHERITED_VARS variable the shell
 *  sets too (inheritedUnsetCommand): the ports ride with the container names,
 *  and a primary compose `up` with a worktree's TALARIA_PG_PORT would
 *  republish main's postgres there; the worktree's ui/.env lines ride with
 *  them, and the primary's `talaria dev` would let them win. */
export function sideStackRefusal(ctx: Ctx, found: readonly SideStackOverride[], command?: string): string {
  const lines = found.map((f) => `    ${f.key}=${f.value}   (${f.whose})`)
  return (
    "the environment names another stack's container where this command means the primary dev stack's (main's):\n" +
    `${lines.join('\n')}\n` +
    "  A worktree's `talaria dev` sets these for itself, and every process it starts (its app, harness runs, their shells) " +
    "inherits them. Acting on them here would read or start that stack's container as if it were main's, so this refuses rather than guess.\n" +
    `  Run it without them: ${inheritedUnsetCommand(ctx, found.map((f) => f.key), command)}`
  )
}

/** Refuse when the shell names, for any of `keys`, a container that is
 *  another stack's: by its shape (sideStackContainers), or because docker
 *  says it belongs to a worktree's or a box's compose project. Every command
 *  that reads or starts MAIN's containers through these variables calls it
 *  before its first docker call that could act. The primary's own override
 *  (a container of its own name, outside any side project) passes. */
export async function refuseSideStackContainers(ctx: Ctx, keys: readonly PrimaryContainerVar[], command: string): Promise<void> {
  const found = sideStackContainers(ctx, keys)
  for (const key of keys) {
    const value = ctx.env[key]
    if (!value || found.some((f) => f.key === key)) continue
    const project = await containerProject(ctx, value)
    if (project && isSideProject(project)) found.push({ key, value, whose: `a container of compose project ${project}` })
  }
  if (found.length > 0) ctx.log.die(sideStackRefusal(ctx, found, command))
}

/** One line naming the stack, for the plan a destructive command prints
 *  before it asks. */
export function describeStack(stack: DevStack): string {
  return stack.kind === 'worktree' ? `worktree "${stack.worktree}" (its own stack, not main's)` : 'the primary dev stack'
}

/** The host port a postgres:// URL names, 5432 when it names none. */
export function urlPort(url: string): string | null {
  try {
    return new URL(url).port || '5432'
  } catch {
    return null
  }
}

type Inspect = {
  State?: { Running?: boolean }
  Config?: { Labels?: Record<string, string> | null }
  NetworkSettings?: { Ports?: Record<string, { HostIp?: string; HostPort?: string }[] | null> | null }
}

/** The database and role every dev stack's postgres serves (dev-compose and
 *  `talaria worktree` both create them), and the ones `reset` execs psql as
 *  (`-U talaria -d talaria`, `drop database talaria`). */
export const DEV_DB = 'talaria'

/** Prove the postgres container this stack names is the database
 *  DATABASE_URL names, before anything is exec'd into it. DATABASE_URL must
 *  name the `talaria` database and role, the ones a command execs psql as
 *  inside the container; the container must be running; a worktree's must
 *  belong to the worktree's own compose project, and the primary's must not
 *  belong to any worktree's; and it must publish DATABASE_URL's port. Any
 *  other answer dies. Returns the published port. */
export async function verifyPostgres(ctx: Ctx, stack: DevStack, dbUrl: string): Promise<string> {
  const name = stack.pgContainer
  let db = ''
  let user = ''
  try {
    const u = new URL(dbUrl)
    db = decodeURIComponent(u.pathname.replace(/^\//, ''))
    user = decodeURIComponent(u.username)
  } catch {
    // unparseable: the names stay empty and the refusal below says so
  }
  if (db !== DEV_DB || user !== DEV_DB) {
    ctx.log.die(
      `DATABASE_URL names database '${db || '(none)'}' as role '${user || '(none)'}', but this command runs psql in ${name} ` +
        `as role '${DEV_DB}' on database '${DEV_DB}'. That is not the database DATABASE_URL names, so refusing rather than guess.`,
    )
  }
  let info: Inspect | undefined
  try {
    info = (JSON.parse((await ctx.exec('docker', ['inspect', name])).stdout) as Inspect[])[0]
  } catch {
    info = undefined
  }
  if (!info?.State?.Running) {
    ctx.log.die(`postgres container '${name}' is not running — start it with \`bun talaria dev\``)
  }
  const project = info.Config?.Labels?.[COMPOSE_PROJECT_LABEL] ?? ''
  if (stack.kind === 'worktree' && project !== stack.infraProject) {
    ctx.log.die(
      `${name} belongs to compose project '${project || '(none)'}', not worktree "${stack.worktree}"'s ${stack.infraProject}. ` +
        'Refusing: it is not this stack\'s database.',
    )
  }
  if (stack.kind === 'primary' && isSideProject(project)) {
    const kind = project.startsWith('devbox-') ? 'devbox' : 'worktree'
    ctx.log.die(`${name} belongs to ${kind} project '${project}', and this is the primary checkout. Refusing.`)
  }
  const want = urlPort(dbUrl)
  const published = (info.NetworkSettings?.Ports?.['5432/tcp'] ?? []).map((b) => b.HostPort ?? '').filter(Boolean)
  if (!want || !published.includes(want)) {
    ctx.log.die(
      `DATABASE_URL names port ${want ?? '(unparseable)'}, but ${name} publishes ${published.length ? published.join(', ') : 'no port'}. ` +
        'They are not the same database, so refusing rather than guess which one you meant.',
    )
  }
  return want
}

/** Every container of a compose project, picked by the project LABEL, never
 *  by a name prefix: a prefix that fits main's fleet fits nothing else, and a
 *  prefix loose enough to fit a worktree's fits main's too. Running only,
 *  unless `all`. A docker that refuses to list answers [], since callers
 *  report what they found. */
export async function projectContainers(ctx: Ctx, project: string, all = false): Promise<string[]> {
  const args = ['ps', ...(all ? ['-a'] : []), '--filter', `label=${COMPOSE_PROJECT_LABEL}=${project}`, '--format', '{{.Names}}']
  try {
    return (await ctx.exec('docker', args)).stdout.split('\n').map((l) => l.trim()).filter(Boolean)
  } catch {
    return []
  }
}

/** The top-level `name:` a rendered compose file declares, or null. The
 *  renderer writes the fleet project there (talaria-fleet-render). */
export function composeFileProject(text: string): string | null {
  const m = /^name:[ \t]*(.+?)[ \t]*$/m.exec(text)
  if (!m) return null
  const v = m[1]!
  return v.length >= 2 && (v[0] === '"' || v[0] === "'") && v.endsWith(v[0]) ? v.slice(1, -1) : v
}

/** `p` (absolute) in its physical spelling: the nearest ancestor that exists
 *  is realpath'd and the rest re-appended, so a path that does not exist yet
 *  still compares against a realpath'd root. This host's /home/jon/Development
 *  is a symlink, and the two spellings of one dir must not read as two. */
export function physical(p: string): string {
  const rest: string[] = []
  let d = p
  for (let i = 0; i < 64; i++) {
    try {
      return join(realpathSync(d), ...rest.reverse())
    } catch {
      const parent = dirname(d)
      if (parent === d) return p
      rest.push(d.slice(parent.length).replace(/^\//, ''))
      d = parent
    }
  }
  return p
}

/** Is `path` inside `dir`, both absolute, compared in their physical
 *  spellings? */
export function inside(dir: string, path: string): boolean {
  const root = physical(dir)
  const p = physical(path)
  return p === root || p.startsWith(root.endsWith('/') ? root : `${root}/`)
}
