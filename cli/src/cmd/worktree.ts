// `talaria worktree` — an ISOLATED dev worktree: its own git worktree, its own
// Postgres + Redis (seeded from the main dev DB), its own Rust api, and its own
// ui/.env on unique ports. It shares nothing mutable with the main environment,
// so you can hack in it without any risk of breaking your primary dev stack.
// Port of scripts/worktree.sh.

import { copyFileSync, existsSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import { compose, stackComposeFiles, waitFor } from '../compose'
import { envValue } from '../envfile'
import { PG_CONTAINER } from '../containers'
import { NAME_RE, portTaken } from '../paths'

/** The shared port slot: first c in 1..89 with app/api/pg/redis
 *  (53xx/54xx/56xx/65xx) all free, so any number of worktrees run at once
 *  without colliding. Injectable probe for tests.
 *
 *  THE API PORT IS IN HERE FOR A REASON. It was not, and the Rust api stayed on
 *  the fixed :5274 for every stack — while `talaria dev` ADOPTS an api already
 *  listening there rather than double-binding. So the second worktree to start
 *  silently proxied every /api/* request to the FIRST worktree's api, which is
 *  pointed at the first worktree's database. Two app servers, one database, by
 *  default — the exact thing docs/WORKTREES.md and AGENTS.md forbid, and
 *  invisible, because the app answered normally with someone else's data. */
export async function worktreeSlot(
  taken: (port: number) => Promise<boolean> = (p) => portTaken(p),
): Promise<{ app: number; api: number; pg: number; redis: number } | null> {
  for (let c = 1; c <= 89; c++) {
    if (
      !(await taken(5300 + c)) &&
      !(await taken(5400 + c)) &&
      !(await taken(5600 + c)) &&
      !(await taken(6500 + c))
    ) {
      return { app: 5300 + c, api: 5400 + c, pg: 5600 + c, redis: 6500 + c }
    }
  }
  return null
}

/** Every key the generated block below writes. The copy of main's ui/.env is
 *  filtered by THIS list — not a hand-kept one — because envValue returns the
 *  FIRST match, so any of these inherited from the source env shadows the line
 *  appended for this stack. Twice that was a hand-kept list one key short:
 *  TALARIA_API_PORT (the api stayed on the shared :5274), then
 *  TALARIA_RUST_API_URL (the port moved, the url the proxy dials did not, and
 *  the app sent every /api/* to the primary's api). A devbox's ui/.env carries
 *  TALARIA_GATEWAY_SELF_URL and TALARIA_FLEET_PROJECT too, so a worktree cut
 *  from inside a box would have handed its agents to the box's app the same
 *  way. Deriving the filter from what is written closes the class. */
export const WORKTREE_ENV_KEYS = [
  'TALARIA_WORKTREE',
  'DATABASE_URL',
  'REDIS_URL',
  'PORT',
  'TALARIA_API_PORT',
  'TALARIA_RUST_API_URL',
  'TALARIA_GATEWAY_SELF_URL',
  'TALARIA_FLEET_PROJECT',
] as const

/** Main's ui/.env with every line this stack rewrites removed. */
export function stripOwnKeys(uiEnv: string): string {
  return uiEnv
    .split('\n')
    .filter((l) => !WORKTREE_ENV_KEYS.some((k) => l.startsWith(`${k}=`)))
    .join('\n')
}

/** The fleet config plane for a checkout, as `talaria setup` seeds it for the
 *  primary: the three dirs the renderer writes into, `fleet/chassis.yml` from
 *  the template, and a `fleet/.env` for the per-agent keys. The chassis is
 *  user-owned config, so an existing one is kept. `fleet/` is gitignored, so a
 *  fresh checkout never has it — and without the chassis the first
 *  `POST /api/fleet/render` 500s with "fleet chassis missing" until someone
 *  copies the template by hand, which is what this step replaces. */
export function seedFleetPlane(ctx: Ctx, templateRoot: string, dir: string): void {
  for (const d of ['fleet/skills', 'fleet/hooks', 'fleet/agents']) mkdirSync(join(dir, d), { recursive: true })
  const chassisTemplate = join(templateRoot, 'scripts/chassis.template.yml')
  const chassisPath = join(dir, 'fleet/chassis.yml')
  if (existsSync(chassisPath)) {
    ctx.log.skip('fleet/chassis.yml (exists, kept)')
  } else if (existsSync(chassisTemplate)) {
    copyFileSync(chassisTemplate, chassisPath)
    ctx.log.ok('fleet/chassis.yml (from template)')
  } else {
    // Say so rather than skip silently: a missing chassis is the exact 500
    // this step exists to prevent, and the template is tracked in the
    // checkout this ran from.
    ctx.log.warn(`no ${chassisTemplate} — fleet/chassis.yml not seeded; renders will fail until it exists`)
  }
  const fleetEnvPath = join(dir, 'fleet/.env')
  if (!existsSync(fleetEnvPath)) {
    writeFileSync(
      fleetEnvPath,
      `# Fleet env — compose interpolation + per-agent HERMES_KEY_* (Talaria appends
# those automatically when you create agents). The gateway brain is
# provisioned here by the first render.
LLM_BASE_URL=
LLM_API_KEY=
LLM_MODEL=
`,
      { mode: 0o600 },
    )
  }
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
  const mainPgc = ctx.env.TALARIA_PG_CONTAINER ?? PG_CONTAINER
  try {
    await ctx.exec('docker', ['inspect', mainPgc])
  } catch {
    ctx.log.die(`main Postgres (${mainPgc}) isn't running — start the main stack first`)
  }
  // `inspect` only proves the CONTAINER exists. The seed below runs pg_dump
  // against the `talaria` DATABASE, and a just-started postgres has not created
  // it yet (the image's entrypoint does that during init), so a worktree
  // created moments after `talaria dev` died with
  //   psql: FATAL: database "talaria" does not exist
  // half way through — after the git worktree and both containers existed, and
  // before ui/.env was written, which is the one step that makes the checkout
  // usable. `pg_isready -d` is the honest precondition: it answers for the
  // database this is about to read, not just the process.
  if (
    !(await waitFor(
      ctx,
      'main postgres',
      async () => {
        try {
          await ctx.exec('docker', ['exec', mainPgc, 'pg_isready', '-U', 'talaria', '-d', 'talaria'])
          return true
        } catch {
          return false
        }
      },
      30,
    ))
  ) {
    ctx.log.die(
      `main Postgres (${mainPgc}) is up but its 'talaria' database never became ready — is the main stack mid-boot, or did setup not finish?`,
    )
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
  // ONLY Postgres and Redis. The dev sidecars (qdrant/embeddings/storage)
  // carry fixed container names and fixed host ports, so a second project
  // cannot bring them up beside the main stack — `up` with no service list dies
  // on the name conflict whenever main is running (the bash script's latent
  // bug; its own header says the worktree owns "its own Postgres + Redis").
  // The worktree app reaches main's sidecars via the TALARIA_*_URL lines that
  // ride in ui/.env below.
  if ((await compose(ctx, { files: stackComposeFiles(root, 'docker/dev-compose.yml'), project }, ['up', '-d', 'postgres', 'redis'])) !== 0) {
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
  // Every key the block below writes is stripped first — WORKTREE_ENV_KEYS
  // says why, and why the list is derived rather than kept by hand.
  const kept = stripOwnKeys(uiEnv)
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
# The Rust api's own port for THIS stack. Without it every worktree's api wants
# the same :5274, and \`talaria dev\` adopts whichever one got there first — so
# this app would proxy /api/* to another worktree's api, on another worktree's
# database. The api binds it (talaria-config reads TALARIA_API_PORT) and the
# app's proxy dials it (TALARIA_RUST_API_URL derives from it in dev.ts).
TALARIA_API_PORT=${slot.api}
# And the url the app's proxy dials, written rather than derived: dev.ts only
# derives it when no env names it, so an inherited line would win over the
# port above.
TALARIA_RUST_API_URL=http://127.0.0.1:${slot.api}
# The fleet THIS worktree renders. Its agents call back over the docker host
# bridge to this app's port — the gateway for their thinking, the MCP gateway
# for their tools — and run under their own compose project, so neither the
# main fleet nor the cleanup sweep reads them as something else's. dev.ts
# lifts both into the api (the renderer reads them; nothing else does).
TALARIA_GATEWAY_SELF_URL=http://host.docker.internal:${slot.app}/api/llm/v1
TALARIA_FLEET_PROJECT=${project}-fleet
`,
  )
  ctx.log.ok('ui/.env written')

  // The fleet config plane, as `talaria setup` seeds it for the primary
  // checkout: the renderer writes into it, and a hire against a missing
  // fleet/.env fails honestly rather than inventing one.
  ctx.log.say('Fleet config plane (fleet/)')
  seedFleetPlane(ctx, root, wt)
  ctx.log.ok('fleet/ seeded')

  ctx.log.say('Sharing node_modules')
  if (!existsSync(join(wt, 'ui/node_modules'))) {
    symlinkSync(join(root, 'ui/node_modules'), join(wt, 'ui/node_modules'))
  }
  ctx.log.ok('linked')

  ctx.log.raw(`
Worktree "${name}" ready — fully isolated from main.

  Run it:   cd ${wt} && bun talaria dev
  App:      http://localhost:${slot.app}   (api :${slot.api} · Postgres :${slot.pg} · Redis :${slot.redis})

  Tear down when done:
    docker compose -p ${project} down -v
    git worktree remove --force ${wt} && git branch -D wt/${name}
  Or later, from the primary checkout:  bun talaria cleanup
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
