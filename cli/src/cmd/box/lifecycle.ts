// `talaria box stop|start|rm|build` — the box lifecycle. Port of the
// corresponding arms of scripts/devbox.

import { existsSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { compose } from '../../compose'
import { boxComposeSpec, boxDir, IMAGE, requireBox } from './shared'
import { runBuild } from './new'

export function runStop(ctx: Ctx, name: string): Promise<number> {
  requireBox(ctx, name)
  return compose(ctx, boxComposeSpec(ctx, name), ['stop'])
}

export async function runStart(ctx: Ctx, name: string): Promise<number> {
  requireBox(ctx, name)
  // The shared stateless services are outside this project — nudge them (the
  // box would run degraded without them, but that's the `talaria dev` posture).
  const devSpec = { files: [join(ctx.root, 'docker/dev-compose.yml')] }
  for (const svc of ['embeddings', 'searxng']) {
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
  ctx.log.say(`Tearing down devbox ${name} (project, volumes, fleet, directory)`)
  // Each teardown step is best-effort (`|| true` in the bash): the goal is
  // the directory gone, and a step that already ran itself clean — or failed
  // on a half-torn-down box — must not shield the rest.
  await compose(ctx, boxComposeSpec(ctx, name), ['down', '-v', '--remove-orphans']).catch(() => {})
  await ctx
    .exec('docker', ['compose', '-p', `devbox-${name}-fleet`, 'down', '-v', '--remove-orphans'])
    .catch(() => {})
  // The fleet network outlives both projects: the box project can't remove it
  // while fleet containers hold endpoints on it, and the fleet compose declares
  // it EXTERNAL (the renderer's shape) so its own `down` never touches it.
  await ctx.exec('docker', ['network', 'rm', `devbox-${name}-fleet`]).catch(() => {})
  rmSync(box, { recursive: true, force: true })
  ctx.log.ok('gone')
  return 0
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
  summary: 'restart a stopped devbox (and the shared TEI/SearXNG)',
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
