// `talaria deploy` actions — the production compose wrappers. Plain
// `docker compose -f docker/compose.yml …` is the CANONICAL operator path
// (docs/CONTAINER.md); each leaf here runs exactly that argv — same relative
// -f, cwd the repo root, interpolation from docker/.env which compose loads
// automatically (the project directory is the compose file's own) — and
// prints the command before running it, so nothing the CLI does is hidden
// and every step stays copy-pasteable. A checkout is required either way
// (the image builds from the repo); bun is the only extra prerequisite.

import { existsSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { envWins, parseEnv } from '../../envfile'

const FILE = 'docker/compose.yml'
const DOCKER_SOCK = '/var/run/docker.sock'

/** The documented invocation. Relative -f on purpose: it is what
 *  CONTAINER.md tells operators to type, and keeping the real argv and the
 *  printed equivalent literally the same string is what makes the print
 *  honest — which is also why this doesn't go through compose()'s helper
 *  (absolute paths, cwd-inherited): parity beats reuse here. */
function deployCompose(ctx: Ctx, op: string[]): Promise<number> {
  return ctx.run('docker', ['compose', '-f', FILE, ...op], { cwd: ctx.root })
}

/** The copy-pasteable line for what is about to run. */
const plain = (envPrefix: string[], op: string[]): string =>
  [...envPrefix, 'docker', 'compose', '-f', FILE, ...op].join(' ')

/** docker/.env as compose will interpolate it, when present. */
export function dockerEnvFile(ctx: Ctx): Record<string, string> {
  const p = join(ctx.root, 'docker/.env')
  return existsSync(p) ? parseEnv(readFileSync(p, 'utf8')) : {}
}

/** GID of the docker socket — the `stat -c %g` half of the documented
 *  command. Null when the socket isn't there; never fatal on its own (the
 *  compose default of 999 applies, and a wrong guess fails loudly at the
 *  mount, not silently here). Injectable path for tests. */
export function socketGid(sock: string = DOCKER_SOCK): string | null {
  try {
    return String(statSync(sock).gid)
  } catch {
    return null
  }
}

/** Resolve DOCKER_GID unless the operator already supplied one — the shell
 *  and docker/.env are the override points, the CLI only fills the gap.
 *  Returns the env prefix the CLI itself added, so the printed equivalent
 *  shows exactly what a shell would have needed. */
function ensureDockerGid(ctx: Ctx, sock: string): string[] {
  if (ctx.env.DOCKER_GID || dockerEnvFile(ctx).DOCKER_GID) return []
  const gid = socketGid(sock)
  if (gid === null) {
    ctx.log.warn(`couldn't stat ${DOCKER_SOCK} — leaving DOCKER_GID to compose's default (999)`)
    return []
  }
  ctx.env.DOCKER_GID = gid
  return [`DOCKER_GID=${gid}`]
}

export function runUp(ctx: Ctx, sock: string = DOCKER_SOCK): Promise<number> {
  const prefix = ensureDockerGid(ctx, sock)
  const op = ['up', '-d', '--build']
  ctx.log.say(plain(prefix, op))
  return deployCompose(ctx, op)
}

export function runDown(ctx: Ctx, volumes: boolean): Promise<number> {
  const op = volumes ? ['down', '--volumes'] : ['down']
  ctx.log.say(plain([], op))
  return deployCompose(ctx, op)
}

/** CONTAINER.md's update flow is "a redeploy" — for a checkout-driven host
 *  that means get the new code, then the same `up -d --build` as boot. The
 *  pull is --ff-only: an update must never synthesize a merge commit on a
 *  deploy host. */
export async function runUpdate(ctx: Ctx, sock: string = DOCKER_SOCK): Promise<number> {
  ctx.log.say('git pull --ff-only')
  if ((await ctx.run('git', ['pull', '--ff-only'], { cwd: ctx.root })) !== 0) {
    ctx.log.die(
      'git pull failed — reconcile the checkout (or fetch/checkout your way), then finish with `bun talaria deploy up`',
    )
  }
  return runUp(ctx, sock)
}

export function runLogs(ctx: Ctx): Promise<number> {
  const op = ['logs', '-f']
  ctx.log.say(plain([], op))
  ctx.log.skip('following every service — Ctrl-C to detach')
  return deployCompose(ctx, op)
}

/** First-run access, the documented way: there are no generated credentials
 *  anymore — a fresh instance is CLAIMED. Point the operator at the claim
 *  screen (same port resolution as runStatus, so the URL is the one compose
 *  actually listens on). */
export async function runCreds(ctx: Ctx): Promise<number> {
  const effective = envWins(dockerEnvFile(ctx), ctx.env)
  const port = effective.TALARIA_HTTP_PORT ?? '5273'
  ctx.log.say(`first-run access: open http://localhost:${port} and claim the admin account`)
  ctx.log.say('the account you create there is the admin — there are no default credentials')
  return 0
}

/** The effective instance identity — env + docker/.env merged the way
 *  compose interpolates — then `ps`. The multi-instance traps in
 *  CONTAINER.md are all "which values did this up actually use", so status
 *  answers that in one line before the container table. */
export async function runStatus(ctx: Ctx): Promise<number> {
  const effective = envWins(dockerEnvFile(ctx), ctx.env)
  const port = effective.TALARIA_HTTP_PORT ?? '5273'
  const state = effective.TALARIA_STATE_DIR ?? '/var/lib/talaria'
  const fleet = `${effective.TALARIA_FLEET_PROJECT ?? 'talaria-fleet'}/${effective.TALARIA_FLEET_NETWORK ?? 'talaria'}`
  ctx.log.say(`http://localhost:${port} · state ${state} · fleet ${fleet}`)
  ctx.log.say(plain([], ['ps']))
  return deployCompose(ctx, ['ps'])
}

export const upCommand: Leaf = {
  kind: 'leaf',
  name: 'up',
  summary: "build + start the stack — CONTAINER.md's one command, DOCKER_GID resolved",
  usage: 'talaria deploy up',
  run: (ctx) => runUp(ctx),
}

export const downCommand: Leaf = {
  kind: 'leaf',
  name: 'down',
  summary: 'stop the stack (--volumes also deletes its data — destructive)',
  usage: 'talaria deploy down [--volumes]',
  flags: [
    {
      name: 'volumes',
      kind: 'bool',
      desc: 'also remove named volumes: pg-data, qdrant-data, minio-data — the DATABASE goes with them',
    },
  ],
  run: (ctx, args) => runDown(ctx, args.flags.volumes === true),
}

export const updateCommand: Leaf = {
  kind: 'leaf',
  name: 'update',
  summary: 'git pull --ff-only, then the redeploy (up -d --build)',
  usage: 'talaria deploy update',
  run: (ctx) => runUpdate(ctx),
}

export const logsCommand: Leaf = {
  kind: 'leaf',
  name: 'logs',
  summary: "follow the stack's logs (Ctrl-C to detach)",
  usage: 'talaria deploy logs',
  run: (ctx) => runLogs(ctx),
}

export const credsCommand: Leaf = {
  kind: 'leaf',
  name: 'creds',
  summary: 'where first-run access lives: the claim screen, not a generated password',
  usage: 'talaria deploy creds',
  run: (ctx) => runCreds(ctx),
}

export const statusCommand: Leaf = {
  kind: 'leaf',
  name: 'status',
  summary: 'effective port/state/fleet + compose ps',
  usage: 'talaria deploy status',
  run: (ctx) => runStatus(ctx),
}
