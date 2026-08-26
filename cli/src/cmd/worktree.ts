// `talaria worktree` — an ISOLATED dev worktree: its own git worktree, its own
// Postgres + Redis (seeded from the main dev DB), its own ui/.env on unique
// ports. It shares nothing mutable with the main environment, so you can hack
// in it without any risk of breaking your primary dev stack. Port of
// scripts/worktree.sh.

import { existsSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import { compose, waitFor } from '../compose'
import { envValue } from '../envfile'
import { NAME_RE, portTaken } from '../paths'

/** The shared port slot: first c in 1..89 with app/pg/redis (53xx/56xx/65xx)
 *  all free, so any number of worktrees run at once without colliding.
 *  Injectable probe for tests. */
export async function worktreeSlot(
  taken: (port: number) => Promise<boolean> = (p) => portTaken(p),
): Promise<{ app: number; pg: number; redis: number } | null> {
  for (let c = 1; c <= 89; c++) {
    if (!(await taken(5300 + c)) && !(await taken(5600 + c)) && !(await taken(6500 + c))) {
      return { app: 5300 + c, pg: 5600 + c, redis: 6500 + c }
    }
  }
  return null
}

export async function runWorktree(ctx: Ctx, name: string, base = 'HEAD'): Promise<number> {
  const root = ctx.root
  if (!NAME_RE.test(name)) ctx.log.die('name must be lowercase-kebab')
  const slot = await worktreeSlot()
  if (!slot) ctx.log.die('no free port slot — tear down some worktrees (docker compose -p talaria-wt-<n> down -v)')
  const wt = join(root, '..', `talaria-${name}`)
  const project = `talaria-wt-${name}`
  const pgc = `talaria-pg-${name}`
  const redisc = `talaria-redis-${name}`

  try {
    await ctx.exec('docker', ['--version'])
  } catch {
    ctx.log.die('docker is required')
  }
  if (!existsSync(join(root, 'ui/.env'))) {
    ctx.log.die('run `bun talaria setup` in the main checkout first (need ui/.env)')
  }
  const mainPgc = ctx.env.TALARIA_PG_CONTAINER ?? 'talaria-postgres-dev'
  try {
    await ctx.exec('docker', ['inspect', mainPgc])
  } catch {
    ctx.log.die(`main Postgres (${mainPgc}) isn't running — start the main stack first`)
  }
  if (existsSync(wt)) ctx.log.die(`${wt} already exists`)

  ctx.log.say(`Git worktree (${wt}, branch wt/${name})`)
  try {
    await ctx.exec('git', ['worktree', 'add', wt, '-b', `wt/${name}`, base], { timeoutMs: 120_000 })
  } catch {
    ctx.log.die(`git worktree add failed (bad base ref ${base}?)`)
  }
  ctx.log.ok('worktree created')

  ctx.log.say(`Isolated infra — Postgres :${slot.pg}, Redis :${slot.redis}`)
  // Interpolation overrides, the bash env-prefix shape. They land in ctx.env
  // (the process env docker compose interpolates from) and stay for this
  // short-lived process — same lifetime as the bash exports.
  ctx.env.TALARIA_PG_CONTAINER = pgc
  ctx.env.TALARIA_PG_PORT = String(slot.pg)
  ctx.env.TALARIA_REDIS_CONTAINER = redisc
  ctx.env.TALARIA_REDIS_PORT = String(slot.redis)
  // ONLY Postgres and Redis. The dev sidecars (qdrant/embeddings/minio/searxng)
  // carry fixed container names and fixed host ports, so a second project
  // cannot bring them up beside the main stack — `up` with no service list dies
  // on the name conflict whenever main is running (the bash script's latent
  // bug; its own header says the worktree owns "its own Postgres + Redis").
  // The worktree app reaches main's sidecars via the TALARIA_*_URL lines that
  // ride in ui/.env below.
  if ((await compose(ctx, { files: [join(root, 'docker/dev-compose.yml')], project }, ['up', '-d', 'postgres', 'redis'])) !== 0) {
    ctx.log.die('worktree infra failed to start')
  }
  await waitFor(
    ctx,
    'worktree postgres',
    async () => {
      try {
        await ctx.exec('docker', ['exec', pgc, 'pg_isready', '-U', 'talaria', '-d', 'talaria'])
        return true
      } catch {
        return false
      }
    },
    40,
  )
  ctx.log.ok('infra up')

  ctx.log.say('Seeding the DB from your main environment')
  try {
    await ctx.pipe(
      ['docker', ['exec', mainPgc, 'pg_dump', '-U', 'talaria', '-d', 'talaria', '--clean', '--if-exists']],
      ['docker', ['exec', '-i', pgc, 'psql', '-U', 'talaria', '-d', 'talaria', '-q']],
      { quietDst: true },
    )
  } catch (e) {
    ctx.log.die(`seed dump/restore failed: ${e instanceof Error ? e.message : String(e)}`)
  }
  ctx.log.ok('seeded (a point-in-time copy of main)')

  ctx.log.say('Worktree ui/.env (own DB, shared encryption root)')
  // Copy main's env, then repoint state + app port. TALARIA_SECRET_KEY is KEPT
  // so the seeded (encrypted) secrets decrypt in this stack. AUTH_SECRET too.
  const uiEnv = readFileSync(join(root, 'ui/.env'), 'utf8')
  const secretKey = envValue(uiEnv, 'TALARIA_SECRET_KEY')
  const kept = uiEnv
    .split('\n')
    .filter((l) => !/^(DATABASE_URL|REDIS_URL|PORT)=/.test(l))
    .join('\n')
  const note = secretKey
    ? ''
    : '\n# NOTE: main ui/.env has no TALARIA_SECRET_KEY — the KEK falls back to AUTH_SECRET (shared here, so seeded secrets decrypt).'
  // The checkout already has ui/ (git made it); recursive mkdir keeps this
  // write self-sufficient regardless of how the tree above it came to be.
  mkdirSync(join(wt, 'ui'), { recursive: true })
  writeFileSync(
    join(wt, 'ui/.env'),
    `${kept}${note}

# ── isolated worktree "${name}" (generated by \`talaria worktree\`) ──
# This marker tells \`talaria dev\` the worktree has its own stack; without it,
# dev refuses to run in a linked worktree (so a plain \`git worktree add\` can't
# point a second app at the main DB). See docs/WORKTREES.md.
TALARIA_WORKTREE=${name}
DATABASE_URL=postgres://talaria:talaria@127.0.0.1:${slot.pg}/talaria
REDIS_URL=redis://127.0.0.1:${slot.redis}
PORT=${slot.app}
`,
  )
  ctx.log.ok('ui/.env written')

  ctx.log.say('Sharing node_modules')
  if (!existsSync(join(wt, 'ui/node_modules'))) {
    symlinkSync(join(root, 'ui/node_modules'), join(wt, 'ui/node_modules'))
  }
  ctx.log.ok('linked')

  ctx.log.raw(`
Worktree "${name}" ready — fully isolated from main.

  Run it:   cd ${wt} && bun talaria dev
  App:      http://localhost:${slot.app}   (Postgres :${slot.pg} · Redis :${slot.redis})

  Tear down when done:
    docker compose -p ${project} down -v
    git worktree remove ${wt} && git branch -D wt/${name}
`)
  return 0
}

export const worktreeCommand: Leaf = {
  kind: 'leaf',
  name: 'worktree',
  summary: 'spin up an isolated worktree stack (own DB seeded from main)',
  usage: 'talaria worktree <name> [base-ref]',
  positionals: { name: 'name', required: true, multiple: true, desc: 'then optionally a base ref (default: HEAD)' },
  run: (ctx, args) => {
    if (args.positionals.length > 2) ctx.log.die(`unexpected argument \`${args.positionals[2]}\``)
    return runWorktree(ctx, args.positionals[0]!, args.positionals[1] ?? 'HEAD')
  },
}
