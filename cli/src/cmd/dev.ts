// `talaria dev` — bring up the whole dev stack: infra (postgres + redis),
// wait until they are actually ready (avoids the cached-migration-failure
// boot gotcha), then the app dev server on :5273. Port of scripts/dev.sh.

import { existsSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import { compose, waitFor } from '../compose'
import { envValue } from '../envfile'
import { anyNewer } from '../paths'
import { renderSearxng } from '../searxng'

const DEV_COMPOSE = 'docker/dev-compose.yml'

/** mcp/ compiles to mcp/dist/index.js, which the app SPAWNS
 *  (ui/src/server/mcp-service.ts) — and mcp/dist is gitignored, so a pull, a
 *  rebase or an edit to mcp/src changes nothing at runtime until it is
 *  rebuilt. Nothing did that before this step, so every agent kept talking to
 *  whatever dist happened to be on disk: stale tool descriptions and, worse,
 *  stale AUTH. Rebuild when dist is missing or ANY source file is newer than
 *  it (`find -newer` semantics: a checkout of an older src still triggers,
 *  because its mtime is the checkout time). TALARIA_SKIP_MCP_BUILD=1 gets
 *  past a build you are mid-way through fixing. */
async function mcpToolkit(ctx: Ctx, opts: { quietWhenFresh?: boolean } = {}): Promise<void> {
  if (ctx.env.TALARIA_SKIP_MCP_BUILD === '1') {
    ctx.log.say('toolkit MCP — skipped (TALARIA_SKIP_MCP_BUILD=1); mcp/dist may be stale')
    return
  }
  if (!existsSync(join(ctx.root, 'mcp/node_modules'))) {
    await ctx.run('bun', ['install'], { cwd: join(ctx.root, 'mcp') })
  }
  const dist = join(ctx.root, 'mcp/dist/index.js')
  const stale = !existsSync(dist) || anyNewer(join(ctx.root, 'mcp/src'), dist)
  if (!stale) {
    if (!opts.quietWhenFresh) ctx.log.say('toolkit MCP — mcp/dist up to date')
    return
  }
  ctx.log.say('toolkit MCP → mcp/dist')
  const code = await ctx.run('bun', ['run', 'build'], { cwd: join(ctx.root, 'mcp') })
  if (code !== 0) {
    ctx.log.die(
      'mcp/ failed to build — mcp/dist is now STALE or missing and the fleet toolkit will ' +
        'serve the old build (or nothing at all). Fix mcp/src, or re-run with ' +
        'TALARIA_SKIP_MCP_BUILD=1 if you meant to leave it.',
    )
  }
}

export async function runDev(ctx: Ctx): Promise<number> {
  const uiEnvPath = join(ctx.root, 'ui/.env')
  if (!existsSync(uiEnvPath)) ctx.log.die('ui/.env missing — run `bun talaria setup` first')
  const uiEnv = readFileSync(uiEnvPath, 'utf8')

  // Guard: a LINKED git worktree (its .git is a file, not a dir) must have
  // its own isolated stack, or it will run a second app against the MAIN dev
  // DB and can corrupt shared state (docs/WORKTREES.md). `talaria worktree`
  // stamps TALARIA_WORKTREE; a plain `git worktree add` won't have it.
  if (isFile(join(ctx.root, '.git')) && !envValue(uiEnv, 'TALARIA_WORKTREE')) {
    ctx.log.die(
      'This is a git worktree without an isolated stack.\n' +
        '  Don\'t run `talaria dev` here — it would share the main dev database.\n' +
        '  Create isolated worktrees with:  bun talaria worktree <name>   (see docs/WORKTREES.md)',
    )
  }

  // Inside a devbox (docs/DEVBOX.md): the box compose owns the infra —
  // sidecars are up, healthy, and reachable by service DNS, and ui/.env
  // already points at them. Bringing up docker/dev-compose.yml through the
  // mounted socket would poke the PRIMARY stack, and the wait loops would
  // block on the wrong containers. Skip straight to what a box still needs:
  // deps, toolkit, app.
  if (ctx.env.TALARIA_DEVBOX) {
    ctx.log.say(`devbox mode (TALARIA_DEVBOX set) — infra owned by the box compose`)
    if (!existsSync(join(ctx.root, 'ui/node_modules'))) {
      await ctx.run('bun', ['install'], { cwd: join(ctx.root, 'ui') })
    }
    await mcpToolkit(ctx, { quietWhenFresh: true })
    ctx.log.say(`app → http://127.0.0.1:${ctx.env.PORT ?? '5273'} (published to the host by the box compose)`)
    return ctx.run('bun', ['run', 'dev'], { cwd: join(ctx.root, 'ui') })
  }

  // A worktree (docs/WORKTREES.md) owns exactly two infra services — its own
  // postgres + redis, created by `talaria worktree` under project
  // talaria-wt-<name>. Aim every compose call and readiness probe at THEM:
  // with the default project/container names this would instead adopt (or
  // start!) the MAIN stack's containers from inside the worktree — the app
  // still read the worktree's DB via ui/.env, but "stop my stack" and the
  // health gates would have been reaching into main. Container names come from
  // the marker, ports from the URLs `talaria worktree` wrote.
  const worktree = envValue(uiEnv, 'TALARIA_WORKTREE')
  if (worktree) {
    ctx.env.TALARIA_PG_CONTAINER ??= `talaria-pg-${worktree}`
    ctx.env.TALARIA_REDIS_CONTAINER ??= `talaria-redis-${worktree}`
    const pgPort = portOf(envValue(uiEnv, 'DATABASE_URL'))
    const redisPort = portOf(envValue(uiEnv, 'REDIS_URL'))
    if (pgPort) ctx.env.TALARIA_PG_PORT ??= pgPort
    if (redisPort) ctx.env.TALARIA_REDIS_PORT ??= redisPort
    ctx.log.say(`infra (this worktree's postgres + redis — sidecars are the main stack's)`)
    const code = await compose(ctx, { files: [join(ctx.root, DEV_COMPOSE)], project: `talaria-wt-${worktree}` }, ['up', '-d', 'postgres', 'redis'])
    if (code !== 0) ctx.log.die('worktree infra failed to start (run `bun talaria worktree` again? it was torn down?)')
    return await waitThenRunApp(ctx, uiEnv)
  }

  // Built-in object storage creds: compose must match the app, so lift them
  // out of ui/.env for interpolation (both fall back to the same dev
  // defaults). Only if the shell hasn't already exported its own — the
  // environment is the override point, same as for the app.
  for (const varName of ['TALARIA_S3_ACCESS_KEY', 'TALARIA_S3_SECRET_KEY']) {
    const val = envValue(uiEnv, varName)
    if (val && !ctx.env[varName]) ctx.env[varName] = val
  }

  ctx.log.say('infra (postgres + redis + qdrant + minio)')
  const devSpec = { files: [join(ctx.root, DEV_COMPOSE)] }
  if ((await compose(ctx, devSpec, ['up', '-d', 'postgres', 'redis', 'qdrant', 'minio'])) !== 0) {
    ctx.log.die('dev infra failed to start')
  }

  renderSearxng(ctx)

  // Search started separately, and non-fatally: one unpullable image must not
  // abort the services the app cannot boot without. Talaria runs fine with
  // search down — `web_search` reports that live search is unavailable and
  // models are told to say so rather than answer from memory.
  ctx.log.say('web search (SearXNG)')
  if ((await compose(ctx, devSpec, ['up', '-d', 'searxng'])) !== 0) {
    ctx.log.warn('search service failed to start — web_search will report itself unavailable.')
    ctx.log.warn('Continuing without it; see docker/searxng/settings.yml.')
  }

  // Embeddings started separately for the same reason: a single `up` resolves
  // every image before creating any container, so one unpullable/broken image
  // (e.g. #151) would abort postgres/redis/qdrant too. The app boots without
  // embeddings — retrieval just degrades — so failure here warns.
  ctx.log.say('embeddings (TEI)')
  if ((await compose(ctx, devSpec, ['up', '-d', 'embeddings'])) !== 0) {
    ctx.log.warn('embeddings service failed to start — retrieval will be degraded.')
    ctx.log.warn('Continuing without it; see docker/dev-compose.yml + issue #151.')
  }

  return await waitThenRunApp(ctx, uiEnv)
}

/** The shared boot tail: infra readiness, deps, the fleet toolkit, then the
 *  vite process (this function's return IS dev's exit code). */
async function waitThenRunApp(ctx: Ctx, uiEnv: string): Promise<number> {
  ctx.log.say('waiting for postgres…')
  const pg = ctx.env.TALARIA_PG_CONTAINER ?? 'talaria-postgres-dev'
  const pgReady = await waitFor(
    ctx,
    'postgres',
    async () => {
      try {
        await ctx.exec('docker', ['exec', pg, 'pg_isready', '-U', 'talaria', '-d', 'talaria'])
        return true
      } catch {
        return false
      }
    },
    40,
  )
  if (!pgReady) ctx.log.die('postgres never became ready')

  ctx.log.say('waiting for redis…')
  const redis = ctx.env.TALARIA_REDIS_CONTAINER ?? 'talaria-redis-dev'
  // No death on timeout here, deliberately (parity with dev.sh): redis that
  // is merely slow must not abort the boot; the app retries connections.
  await waitFor(
    ctx,
    'redis',
    async () => {
      try {
        await ctx.exec('docker', ['exec', redis, 'redis-cli', 'ping'])
        return true
      } catch {
        return false
      }
    },
    20,
  )

  if (!existsSync(join(ctx.root, 'ui/node_modules'))) {
    await ctx.run('bun', ['install'], { cwd: join(ctx.root, 'ui') })
  }

  await mcpToolkit(ctx)

  // A worktree's ui/.env carries its own PORT (the allocated slot). vite's
  // dev script hardcodes 5273, and a busy port makes vite silently bind the
  // NEXT one — while the worktree's .env URLs keep claiming its own slot —
  // so forward the flag explicitly and keep the two honest with each other.
  const wtPort = envValue(uiEnv, 'TALARIA_WORKTREE') ? envValue(uiEnv, 'PORT') : undefined
  const devArgs = wtPort ? ['run', 'dev', '--', '--port', wtPort] : ['run', 'dev']

  ctx.log.say(`app → http://localhost:${wtPort ?? '5273'}`)
  return ctx.run('bun', devArgs, { cwd: join(ctx.root, 'ui') })
}

/** Host port of a postgres:// or redis:// URL, or null when unparseable. */
function portOf(url: string | undefined): string | null {
  if (!url) return null
  try {
    return new URL(url).port || null
  } catch {
    return null
  }
}

function isFile(p: string): boolean {
  try {
    return statSync(p).isFile()
  } catch {
    return false
  }
}

export const devCommand: Leaf = {
  kind: 'leaf',
  name: 'dev',
  summary: 'run the dev stack: infra, readiness waits, then the app on :5273',
  usage: 'talaria dev',
  run: runDev,
}
