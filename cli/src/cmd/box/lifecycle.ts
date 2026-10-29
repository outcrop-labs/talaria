// `talaria box stop|start|rm|build` — the box lifecycle. Port of the
// corresponding arms of scripts/devbox.

import { existsSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { compose, stackComposeFiles } from '../../compose'
import { boxComposeSpec, boxDir, boxFleetNetwork, boxNetwork, boxState, IMAGE, requireBox } from './shared'
import { runBuild } from './new'

export function runStop(ctx: Ctx, name: string): Promise<number> {
  requireBox(ctx, name)
  return compose(ctx, boxComposeSpec(ctx, name), ['stop'])
}

export async function runStart(ctx: Ctx, name: string): Promise<number> {
  requireBox(ctx, name)
  // The shared stateless service is outside this project — nudge it (the
  // box would run degraded without it, but that's the `talaria dev` posture).
  const devSpec = { files: stackComposeFiles(ctx.root, 'docker/dev-compose.yml') }
  for (const svc of ['embeddings']) {
    if ((await compose(ctx, devSpec, ['up', '-d', svc])) !== 0) {
      ctx.log.warn(`${svc} didn't start — box runs degraded without it`)
    }
  }
  // `up -d`, NOT `start`: `start` merely re-launches the existing containers —
  // an edited compose.override.yml (the documented channel for auth/env
  // changes: stop, edit, start) would never apply. `up -d` converges: it
  // recreates containers whose config changed and starts the stopped rest.
  return compose(ctx, boxComposeSpec(ctx, name), ['up', '-d', '--quiet-pull'])
}

export async function runRm(ctx: Ctx, name: string, force: boolean): Promise<number> {
  requireBox(ctx, name)
  const box = boxDir(ctx, name)
  // Protect the work: a dirty tree or commits no remote has are the only
  // things in a box that can't be recreated.
  if (!force && existsSync(join(box, 'talaria/.git'))) {
    const dirty = await (async () => {
      try {
        return (await ctx.exec('git', ['-C', join(box, 'talaria'), 'status', '--porcelain'])).stdout
      } catch {
        return '' // git refusing to run is not a reason to block teardown
      }
    })()
    if (dirty.length > 0) ctx.log.die('clone has uncommitted changes — commit/stash them, or --force')
    let unpushed = ''
    try {
      unpushed = (await ctx.exec('git', ['-C', join(box, 'talaria'), 'log', '--branches', '--not', '--remotes', '--oneline'])).stdout.trim()
    } catch {
      unpushed = ''
    }
    if (unpushed) {
      ctx.log.die(`clone has commits no remote has:
${unpushed}
push them (or --force to discard)`)
    }
  }
  ctx.log.say(`Tearing down devbox ${name} (project, volumes, fleet, app databases, directory)`)
  // Each teardown step is best-effort (`|| true` in the bash): the goal is
  // the directory gone, and a step that already ran itself clean — or failed
  // on a half-torn-down box — must not shield the rest.
  //
  // The app databases go first. Each is a compose project of its own
  // (ui/src/server/app-db.ts) on the box's default network, so the box
  // project's `down` neither removes them nor can remove the network they
  // hold an endpoint on.
  for (const db of await boxAppDbs(ctx, name)) {
    await ctx.exec('docker', ['rm', '-f', '-v', db]).catch(() => {})
  }
  await compose(ctx, boxComposeSpec(ctx, name), ['down', '-v', '--remove-orphans']).catch(() => {})
  await ctx
    .exec('docker', ['compose', '-p', boxFleetNetwork(name), 'down', '-v', '--remove-orphans'])
    .catch(() => {})
  // The fleet network outlives both projects: the box project can't remove it
  // while fleet containers hold endpoints on it, and the fleet compose declares
  // it EXTERNAL (the renderer's shape) so its own `down` never touches it.
  await ctx.exec('docker', ['network', 'rm', boxFleetNetwork(name)]).catch(() => {})
  await clearState(ctx, name)
  try {
    rmSync(box, { recursive: true, force: true })
  } catch (e) {
    const code = (e as NodeJS.ErrnoException).code ?? String(e)
    ctx.log.fail(`${box} is still there (${code}): files a container wrote in it are not yours to delete`)
    ctx.log.say(`  everything else is down; remove it as root: sudo rm -rf ${box}`)
    return 1
  }
  ctx.log.ok('gone')
  return 0
}

/** The app databases a box's app started, by either of two marks, stopped
 *  ones included:
 *  - on the box's default network (`TALARIA_APP_DB_NETWORK` in
 *    docker/devbox.compose.yml), named `talaria-appdb-<box>-<slug>`. Found by
 *    the network, not by a name prefix, because box `a`'s prefix
 *    `talaria-appdb-a-` is also box `a-b`'s; a stopped container keeps its
 *    network;
 *  - with a mount inside the box's state, where its cluster lives
 *    (`<state>/app-data/<slug>/pg`). That finds the ones a box made before it
 *    set the network: `talaria-appdb-1-<slug>` on the default bridge, which
 *    `restart: unless-stopped` would keep running on a directory this
 *    removes.
 *  An answer docker could not give is no databases, like every other
 *  best-effort teardown step. */
export async function boxAppDbs(ctx: Ctx, name: string): Promise<string[]> {
  const ps = async (filters: string[]): Promise<string[]> => {
    try {
      const { stdout } = await ctx.exec('docker', ['ps', '-a', ...filters, '--format', '{{.Names}}'])
      return stdout.split('\n').map((s) => s.trim()).filter(Boolean)
    } catch {
      return []
    }
  }
  const found = new Set(await ps(['--filter', `network=${boxNetwork(name)}`, '--filter', 'name=^talaria-appdb-']))
  const rest = (await ps(['--filter', 'name=^talaria-appdb-'])).filter((n) => !found.has(n))
  if (rest.length > 0) {
    try {
      const { stdout } = await ctx.exec('docker', ['inspect', '-f', '{{.Name}}\t{{json .Mounts}}', ...rest])
      for (const db of mountedUnder(stdout, boxState(ctx, name))) found.add(db)
    } catch {
      /* none found this way */
    }
  }
  return [...found]
}

/** Of `inspect -f '{{.Name}}\t{{json .Mounts}}'` lines, the containers with a
 *  mount source at or under `dir`. Pure. */
export function mountedUnder(inspectLines: string, dir: string): string[] {
  const root = resolve(dir)
  const out: string[] = []
  for (const line of inspectLines.split('\n')) {
    const tab = line.indexOf('\t')
    if (tab < 0) continue
    let mounts: unknown
    try {
      mounts = JSON.parse(line.slice(tab + 1))
    } catch {
      continue
    }
    if (!Array.isArray(mounts)) continue
    const under = mounts.some((m) => {
      const src = (m as { Source?: unknown } | null)?.Source
      if (typeof src !== 'string') return false
      const at = resolve(src)
      return at === root || at.startsWith(`${root}/`)
    })
    if (under) out.push(line.slice(0, tab).replace(/^\//, ''))
  }
  return out
}

/** Empty the box's state directory from a root container on the box's own
 *  image. What a container wrote there is that container's user's: an app
 *  database's cluster is the postgres image's uid 70 at mode 0700, so this
 *  user can neither list nor delete it: the directory removal threw EACCES
 *  and left the box's directory, its stack already down, for every later
 *  `rm` to fail on the same way. No network, and never a pull: the image is
 *  built here (`box build`) and named on no registry, so an image pruned
 *  since the box's container went would have sent `run` to Docker Hub for a
 *  name that is not this one. Best effort like the rest; runRm says so when
 *  the directory outlives it. */
export async function clearState(ctx: Ctx, name: string): Promise<void> {
  const state = boxState(ctx, name)
  if (!existsSync(state)) return
  await ctx
    .exec(
      'docker',
      [
        'run', '--rm', '--pull', 'never', '--network', 'none', '--user', '0:0',
        '-v', `${state}:/state`, '--entrypoint', 'find', IMAGE, '/state', '-mindepth', '1', '-delete',
      ],
      { timeoutMs: 300_000 },
    )
    .catch(() => {})
}

export const stopCommand: Leaf = {
  kind: 'leaf',
  name: 'stop',
  summary: 'stop a devbox\'s containers (state kept)',
  usage: 'talaria box stop <name>',
  positionals: { name: 'name', required: true },
  run: (ctx, args) => runStop(ctx, args.positionals[0]!),
}

export const startCommand: Leaf = {
  kind: 'leaf',
  name: 'start',
  summary: 'restart a stopped devbox (and the shared TEI)',
  usage: 'talaria box start <name>',
  positionals: { name: 'name', required: true },
  run: (ctx, args) => runStart(ctx, args.positionals[0]!),
}

export const rmCommand: Leaf = {
  kind: 'leaf',
  name: 'rm',
  summary: 'tear down a devbox — refuses unpushed work unless --force',
  usage: 'talaria box rm <name> [--force]',
  positionals: { name: 'name', required: true },
  flags: [{ name: 'force', kind: 'bool', desc: 'discard uncommitted changes and unpushed commits' }],
  run: (ctx, args) => runRm(ctx, args.positionals[0]!, args.flags.force === true),
}

export const buildCommand: Leaf = {
  kind: 'leaf',
  name: 'build',
  summary: `rebuild the ${IMAGE} toolchain image`,
  usage: 'talaria box build [--no-cache]',
  flags: [{ name: 'no-cache', kind: 'bool', desc: 'rebuild every layer' }],
  run: (ctx, args) => runBuild(ctx, args.flags['no-cache'] === true),
}
