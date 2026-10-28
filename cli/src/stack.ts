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
//     checkout's DATABASE_URL names: another compose project, or another port.
//
// The names below mirror what cmd/worktree.ts creates for a worktree. Change
// them together.

import { existsSync, realpathSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import type { Ctx } from './ctx'
import { PG_CONTAINER, REDIS_CONTAINER } from './containers'
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
 *  `uiEnv` is ui/.env's text when the caller has already read it. */
export function devStack(ctx: Ctx, uiEnv: string = envFileText(ctx, 'ui/.env')): DevStack {
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
    ctx.log.die(
      `TALARIA_WORKTREE=${shellWorktree} in the environment, but ${join(ctx.root, 'ui/.env')} ` +
        (worktree ? `marks worktree "${worktree}"` : 'carries no worktree marker') +
        '. Refusing rather than guess which stack you meant: unset it, or run from that worktree.',
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
    return {
      kind: 'primary',
      worktree: null,
      pgContainer: ctx.env.TALARIA_PG_CONTAINER ?? PG_CONTAINER,
      redisContainer: ctx.env.TALARIA_REDIS_CONTAINER ?? REDIS_CONTAINER,
      infraProject: PRIMARY_INFRA_PROJECT,
      // The api's own resolution order: the shell, then ui/.env (dev.ts lifts
      // it), then the layout's default.
      fleetProject: ctx.env.TALARIA_FLEET_PROJECT ?? envValue(uiEnv, 'TALARIA_FLEET_PROJECT') ?? PRIMARY_FLEET_PROJECT,
      fleetProblem: null,
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
      `worktree "${worktree}" names no TALARIA_FLEET_PROJECT in ui/.env, so its api rendered into the default project ` +
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

/** Prove the postgres container this stack names is the database
 *  DATABASE_URL names, before anything is exec'd into it. It must be running;
 *  a worktree's must belong to the worktree's own compose project, and the
 *  primary's must not belong to any worktree's; and it must publish
 *  DATABASE_URL's port. Any other answer dies. Returns the published port. */
export async function verifyPostgres(ctx: Ctx, stack: DevStack, dbUrl: string): Promise<string> {
  const name = stack.pgContainer
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
  if (stack.kind === 'primary' && project.startsWith('talaria-wt-')) {
    ctx.log.die(`${name} belongs to worktree project '${project}', and this is the primary checkout. Refusing.`)
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
