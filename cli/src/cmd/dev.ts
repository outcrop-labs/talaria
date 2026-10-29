// `talaria dev` — bring up the whole dev stack: infra (postgres + redis),
// wait until they are actually ready (avoids the cached-migration-failure
// boot gotcha), then the app dev server on :5273. Port of scripts/dev.sh.

import { existsSync, readFileSync, statSync } from 'node:fs'
import { spawn } from 'node:child_process'
import { join } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import { compose, stackComposeFiles, waitFor, waitForPostgres, worktreeComposeFiles } from '../compose'
import { envValue } from '../envfile'
import { anyNewer } from '../paths'
import { API_PORT } from '../ports'
import { ALL_PRIMARY_CONTAINER_VARS, devStack, refuseForeignCheckout, refuseSideStackContainers, refuseWorktreeEnvOverride, type DevStack } from '../stack'

const DEV_COMPOSE = 'docker/dev-compose.yml'

/** mcp/ compiles to mcp/dist/index.js, which the Rust api SPAWNS
 *  (api/src/mcp/service.rs) — and mcp/dist is gitignored, so a pull, a
 *  rebase or an edit to mcp/src changes nothing at runtime until it is
 *  rebuilt. Nothing did that before this step, so every agent kept talking to
 *  whatever dist happened to be on disk: stale tool descriptions and, worse,
 *  stale AUTH. Rebuild when dist is missing or ANY source file is newer than
 *  it (`find -newer` semantics: a checkout of an older src still triggers,
 *  because its mtime is the checkout time). TALARIA_SKIP_MCP_BUILD=1 gets
 *  past a build you are mid-way through fixing. */
export async function mcpToolkit(ctx: Ctx, opts: { quietWhenFresh?: boolean } = {}): Promise<void> {
  if (ctx.env.TALARIA_SKIP_MCP_BUILD === '1') {
    ctx.log.say('toolkit MCP — skipped (TALARIA_SKIP_MCP_BUILD=1); mcp/dist may be stale')
    return
  }
  if (!existsSync(join(ctx.root, 'mcp/node_modules'))) {
    await ctx.run('bun', ['install'], { cwd: join(ctx.root, 'mcp') })
  }
  const dist = join(ctx.root, 'mcp/dist/index.js')
  // PRESENT and STALE are different failures, and the old message conflated
  // them into one `die`. A dist that exists and is merely out of date still
  // SERVES: the api spawns it, agents get the previous build's tools, and the
  // worst case is tool descriptions that lag mcp/src — annoying, not fatal,
  // and not a reason to refuse to bring up a stack somebody is using to work
  // on something else entirely. A dist that is ABSENT serves nothing, so the
  // toolkit is simply gone and a stack that came up anyway would hand every
  // agent an empty tool list and look like a product bug.
  const present = existsSync(dist)
  const stale = !present || anyNewer(join(ctx.root, 'mcp/src'), dist)
  if (!stale) {
    if (!opts.quietWhenFresh) ctx.log.say('toolkit MCP — mcp/dist up to date')
    return
  }
  ctx.log.say('toolkit MCP → mcp/dist')
  const code = await ctx.run('bun', ['run', 'build'], { cwd: join(ctx.root, 'mcp') })
  if (code !== 0) {
    if (!present) {
      ctx.log.die(
        'mcp/ failed to build and mcp/dist does not exist — the fleet toolkit would serve ' +
          'NOTHING, so every agent would come up with an empty tool list. Fix mcp/src, or ' +
          're-run with TALARIA_SKIP_MCP_BUILD=1 if you meant to leave it.',
      )
    }
    ctx.log.warn(
      'mcp/ failed to build — mcp/dist is STALE, so the fleet toolkit keeps serving the ' +
        'PREVIOUS build: agents may see tool descriptions and auth that lag mcp/src. ' +
        'Carrying on; fix mcp/src and re-run to pick it up.',
    )
  }
}

/** omp-auth/ bundles to omp-auth/dist/server.js, which the Rust api SPAWNS
 *  (talaria-omp-auth) to perform coding-account OAuth logins and refreshes.
 *  Same staleness problem as the toolkit above, and the same answer — except
 *  that a missing bundle is NOT fatal here: coding accounts are an opt-in
 *  feature behind an admin toggle, and a dev stack that never turns it on
 *  should not be blocked by a bundle it will never spawn. So a failure warns
 *  and carries on, and the feature's own routes say the bridge is not running
 *  if someone does turn it on. TALARIA_SKIP_OMP_AUTH_BUILD=1 skips it. */
export async function ompAuthBridge(ctx: Ctx, opts: { quietWhenFresh?: boolean } = {}): Promise<void> {
  if (ctx.env.TALARIA_SKIP_OMP_AUTH_BUILD === '1') {
    ctx.log.say('omp auth bridge — skipped (TALARIA_SKIP_OMP_AUTH_BUILD=1)')
    return
  }
  const dist = join(ctx.root, 'omp-auth/dist/server.js')
  const stale = !existsSync(dist) || anyNewer(join(ctx.root, 'omp-auth/src'), dist)
  if (!stale) {
    if (!opts.quietWhenFresh) ctx.log.say('omp auth bridge — omp-auth/dist up to date')
    return
  }
  if (!existsSync(join(ctx.root, 'omp-auth/node_modules'))) {
    await ctx.run('bun', ['install'], { cwd: join(ctx.root, 'omp-auth') })
  }
  ctx.log.say('omp auth bridge → omp-auth/dist')
  const code = await ctx.run('bun', ['run', 'build'], { cwd: join(ctx.root, 'omp-auth') })
  if (code !== 0) {
    ctx.log.warn(
      'omp-auth/ failed to build — coding-account sign-ins will report the bridge as not ' +
        'running. Everything else is unaffected.',
    )
  }
}

const API_ENV_VARS = [
  'DATABASE_URL',
  'REDIS_URL',
  'TALARIA_SECRET_KEY',
  'TALARIA_SECRET_KEY_FILE',
  'AUTH_SECRET',
  'TALARIA_SCHEDULER',
  'SEARXNG_URL',
  'TALARIA_API_PORT',
  'TALARIA_API_BIND',
  'TALARIA_GATEWAY_SELF_URL',
  'PORT',
  'TALARIA_APP_HOST_URL',
  'TALARIA_FLEET_PROJECT',
  'TALARIA_WORKTREE',
  'TALARIA_MCP_PORT',
  'TALARIA_OMP_AUTH_PORT',
  'TALARIA_GATEWAY_PORT_BASE',
  'TALARIA_APPS_LOCAL_SOURCES',
  // A devbox's own block (box/new.ts boxBlock): every line of it the api
  // reads. The last paragraph below says why.
  'TALARIA_AGENT_DIAL',
  'TALARIA_MCP_GW_URL',
  'TALARIA_FLEET_DIR',
  'TALARIA_APPS_DIR',
  'TALARIA_UPLOADS_DIR',
  'TALARIA_QDRANT_URL',
  'TALARIA_EMBED_URL',
  'TALARIA_S3_URL',
  'TALARIA_S3_BUCKET',
  'TALARIA_S3_ACCESS_KEY',
  'TALARIA_S3_SECRET_KEY',
] as const

/** The connection facts the Rust api reads, lifted out of ui/.env with the
 *  SHELL always winning — the same override point `talaria dev` gives every
 *  other lifted var. In a worktree, a shell value that differs from a line
 *  its ui/.env owns never gets this far: runDev refuses it first
 *  (stack.ts refuseWorktreeEnvOverride).
 *
 *  The api's config.rs resolves the secret root in this order; lift all
 *  three candidates so ui/.env's arrangement works whichever one it uses.
 *  TALARIA_SCHEDULER rides along for one reason now: `off` is the kill
 *  switch, and it has to reach the api process to mean anything. (During
 *  the port the same variable declared which RUNTIME armed the schedule;
 *  the cutover deleted the TS scheduler, so the api arms unless this says
 *  `off`.) SEARXNG_URL — an operator's own search endpoint, nothing is
 *  bundled — rides so the cargo child sees the same value the vite process
 *  does (vite loads ui/.env itself) — the Rust read of it decides whether
 *  search is env-pinned, and the two views must agree.
 *  TALARIA_API_BIND and TALARIA_GATEWAY_SELF_URL are the container/devbox
 *  dial: without the lift, ui/.env's 0.0.0.0 bind and :5274 gateway never
 *  reach the renderer, and agents keep queueing on the UI hop.
 *
 *  TALARIA_API_PORT is in this list because the api BINDS it (talaria-config
 *  reads it, defaulting to 5274). Without the lift, a worktree's api would
 *  dutifully bind the shared port while the proxy dialed the worktree's own —
 *  the two halves of the same setting have to agree. PORT and
 *  TALARIA_FLEET_PROJECT are the fleet renderer's: the MCP gateway URL it
 *  writes into every agent is built from the APP port, and the compose
 *  project it addresses is what keeps a worktree's agents apart from the
 *  main fleet — without the lift a worktree's agents would dial :5273 and
 *  its containers would land in the shared `talaria-fleet` project. PORT is
 *  also how the api reaches this app host for app harnesses (fitness reads
 *  them from /api/app-host/harnesses on loopback); TALARIA_APP_HOST_URL
 *  overrides that address outright.
 *
 *  TALARIA_MCP_PORT, TALARIA_OMP_AUTH_PORT and TALARIA_GATEWAY_PORT_BASE are
 *  a worktree's own ports for what the api spawns and renders: the toolkit
 *  MCP child, the omp auth bridge child, and the first agent gateway port.
 *  The api reads all three and nothing else does, so without the lift every
 *  stack's api fell back to :5280, :5276 and :8770 and the second stack
 *  collided with the first (talaria worktree says how).
 *
 *  TALARIA_WORKTREE names the instance in every container the api owns: an
 *  app's database (`talaria-appdb-<instance>-<slug>`) and an MCP package's
 *  (`talaria-mcp-<instance>-<name>`). The app side reads it from ui/.env, so
 *  without the lift the api named the main instance's container — uninstall
 *  stopped a container that was not this worktree's and left its own
 *  running.
 *
 *  TALARIA_APPS_LOCAL_SOURCES=1 lets the api install and update apps from a
 *  repository on this machine (file://, or http:// to loopback) — a dev
 *  stack's way to exercise the git install and update path without a
 *  remote. The api is the one that reads it, so ui/.env has to reach it.
 *
 *  A devbox's block rides for the reason every line here does: the api has
 *  no .env loader, and in a box nothing else hands those lines over. The box
 *  compose's environment carries only TALARIA_DEVBOX and the app database
 *  network, `box enter` sources nothing, and the devbox branch of runDev
 *  skips the S3 lift the primary's compose call makes. So a box's api read
 *  TALARIA_AGENT_DIAL as unset, published loopback ports for its agents and
 *  dialled them on the box's own loopback. It took its fleet dir from its
 *  cwd, /work/talaria/fleet, a path only the box container has: every bind
 *  mount it rendered named a host path that did not exist, which the host's
 *  daemon makes as an empty root-owned directory. It handed agents the
 *  host's MCP gateway URL, and dialled qdrant, embeddings and object storage
 *  on 127.0.0.1. `box/box.test.ts` pins every block key the api reads to
 *  this list. The primary and a worktree write none of these lines but the
 *  S3 pair, which their `talaria dev` already exports before the api starts,
 *  so nothing changes there. */
export const API_ENV_KEYS: readonly string[] = API_ENV_VARS

export function apiEnv(shell: Record<string, string | undefined>, uiEnv: string): Record<string, string> {
  const env: Record<string, string> = {}
  for (const varName of API_ENV_VARS) {
    const val = shell[varName] ?? envValue(uiEnv, varName)
    if (val) env[varName] = val
  }
  return env
}

/** The Rust api sidecar — the app's api since the cutover: every /api/*
 *  request the dev UI serves is proxied to it, so `talaria dev` brings it up
 *  by default. Opt OUT with TALARIA_API=off (a box that runs its own instance
 *  — the sidecar adopts whatever is already listening, so the opt-out is for
 *  "never, on purpose").
 *
 *  The api binds in-box loopback (127.0.0.1:5274 — no published port, no
 *  compose change) and reads the same connection facts the app does, lifted
 *  out of ui/.env with the SHELL always winning — the same override point
 *  `talaria dev` gives every other lifted var.
 *
 *  Non-fatal at every step, on purpose: a box without cargo, or one whose
 *  api fails to build, still gets the UI — but the app is API-DARK without
 *  the sidecar now, so every skip says so at full volume. */
export async function rustApi(ctx: Ctx, uiEnv: string): Promise<void> {
  if (ctx.env.TALARIA_API === 'off') return

  // ui/.env is read here too, not just the shell. A worktree writes its own
  // TALARIA_API_PORT into ui/.env (talaria worktree), and reading only ctx.env
  // meant that file was ignored: the port computed here is what the proxy dials
  // AND what the adopt probe below tests, so missing it sent both back to the
  // shared :5274.
  const port = ctx.env.TALARIA_API_PORT ?? envValue(uiEnv, 'TALARIA_API_PORT') ?? API_PORT
  const url = `http://127.0.0.1:${port}`
  // The proxy defaults to exactly this loopback address, but lift the env
  // anyway when nothing else names it (the shell first, then ui/.env — the
  // S3 lift's precedence) so a custom TALARIA_API_PORT reaches the vite
  // child, and so `talaria dev` never depends on the default being right.
  if (!ctx.env.TALARIA_RUST_API_URL && !envValue(uiEnv, 'TALARIA_RUST_API_URL')) {
    ctx.env.TALARIA_RUST_API_URL = url
  }

  if (await probe(url) !== null) {
    // Already up — a hand-started instance, or a second `talaria dev`. Adopt
    // it: double-binding would just die loudly at the Rust end.
    ctx.log.say(`rust api → already listening on :${port}; adopting it`)
    return
  }
  if (!await hasCargo(ctx)) {
    ctx.log.warn(
      'no cargo on PATH — skipping the Rust api. The UI will serve, but every /api/* request fails:' +
        ' the api is the whole product surface, not a side feature.',
    )
    return
  }

  const env = apiEnv(ctx.env, uiEnv)
  const child = spawn('cargo', ['run', '--quiet'], {
    cwd: join(ctx.root, 'api'),
    env: { ...process.env, ...env },
    stdio: 'inherit',
  })
  child.on('error', () => ctx.log.warn('rust api failed to start — the app has no api until it does; every /api/* request fails.'))
  child.on('close', (code) => { if (code) ctx.log.warn(`rust api exited ${code} — the app has no api until it is restarted.`) })
  // Ctrl-C reaches cargo through the tty's process group; the handlers are
  // the belt-and-braces path, because a LEAKED api holds :5274 and an open
  // pool against the dev DB until the box dies.
  for (const sig of ['SIGINT', 'SIGTERM'] as const) process.on(sig, () => child.kill(sig))
  process.on('exit', () => child.kill('SIGTERM'))
  ctx.log.say(`rust api → cargo run in api/ (a cold build takes minutes); binding :${port}`)
  void pollReady(ctx, url)
}

/** Is something answering HTTP at `url`? The status — whatever it is — or
 *  null when nothing is listening (the adoption check cares that the port is
 *  taken, not that the answer is healthy). */
async function probe(url: string): Promise<number | null> {
  try {
    const res = await fetch(`${url}/api/healthz`, { signal: AbortSignal.timeout(1500) })
    return res.status
  } catch {
    return null
  }
}

/** Say something exactly once, whenever the api is actually serving: a cold
 *  cargo build is minutes, so this must never hold up the app — it runs
 *  alongside vite and speaks when the proxy's target goes live (or 5 minutes
 *  say it never did — the cargo output above holds the reason). */
async function pollReady(ctx: Ctx, url: string): Promise<void> {
  for (let i = 0; i < 300; i++) {
    await new Promise((r) => setTimeout(r, 1000))
    if (await probe(url) === 200) {
      ctx.log.say(`rust api → ${url} ready`)
      return
    }
  }
  ctx.log.warn('rust api never became ready — check the cargo output above.')
}

async function hasCargo(ctx: Ctx): Promise<boolean> {
  try {
    await ctx.exec('cargo', ['--version'])
    return true
  } catch {
    return false
  }
}


/** One line when the disk is over the sweep's line. Never blocks boot —
 *  the stack may be what the task needs in order to finish and free the space.
 *  The policy is scripts/cleanup-sweep.mjs; a probe that cannot run is silence. */
async function warnDiskPressure(ctx: Ctx): Promise<void> {
  try {
    const r = await ctx.exec(process.execPath, [join(ctx.root, 'scripts/cleanup-sweep.mjs'), '--pressure'], {
      timeoutMs: 5_000,
    })
    const line = `${r.stdout}${r.stderr}`.trim()
    if (line) ctx.log.warn(line)
  } catch {
    // a probe that cannot run must not stop the stack
  }
}

/** The port vite's own dev script binds (ui/package.json). */
export const VITE_SCRIPT_PORT = '5273'

/** vite's argv. `--strictPort` always: without it a port already taken made
 *  vite bind the NEXT one and carry on, while everything that names this
 *  stack's app port kept naming the old one — a worktree's ui/.env URLs and
 *  its agents' TALARIA_GATEWAY_SELF_URL, main's agents on :5273, a box's
 *  published port. The stack looked up and its agents could not reach it
 *  (follow-up R23). A worktree passes its own PORT; vite's script hardcodes
 *  5273 for everyone else. */
export function viteArgs(port: string | undefined): string[] {
  return port ? ['run', 'dev', '--', '--port', port, '--strictPort'] : ['run', 'dev', '--', '--strictPort']
}

/** Who is listening on a TCP port: `pid N (command)`, a container that
 *  publishes it, or a plain "something" when this user cannot see the
 *  process. null when nothing is listening — or when neither `ss` nor `lsof`
 *  can say, which leaves the verdict to vite's own --strictPort. */
export async function portHolder(ctx: Ctx, port: string): Promise<string | null> {
  const quick = { timeoutMs: 5_000 }
  let listening = false
  let pid: string | null = null
  let command: string | null = null
  try {
    const out = (await ctx.exec('ss', ['-Hltnp', 'sport', '=', `:${port}`], quick)).stdout.trim()
    listening = out !== ''
    const m = /users:\(\("([^"]*)",pid=(\d+)/.exec(out)
    if (m) [command, pid] = [m[1]!, m[2]!]
  } catch {
    // No ss (macOS): lsof answers the same question. It exits 1 when nothing
    // matches, which is the same answer as no lsof at all.
    try {
      const out = (await ctx.exec('lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-Fpc'], quick)).stdout
      listening = out.trim() !== ''
      pid = /^p(\d+)$/m.exec(out)?.[1] ?? null
      command = /^c(.+)$/m.exec(out)?.[1] ?? null
    } catch {
      return null
    }
  }
  if (!listening) return null
  if (pid) {
    try {
      const args = (await ctx.exec('ps', ['-o', 'args=', '-p', pid], quick)).stdout.trim()
      if (args) command = args.length > 120 ? `${args.slice(0, 117)}...` : args
    } catch {
      // gone already, or ps refused: the name ss gave is still worth printing
    }
    return `pid ${pid}${command ? ` (${command})` : ''}`
  }
  // A process this user cannot see: docker's port proxy is the usual one.
  try {
    const names = (await ctx.exec('docker', ['ps', '--filter', `publish=${port}`, '--format', '{{.Names}}'], quick)).stdout
      .split('\n')
      .map((n) => n.trim())
      .filter(Boolean)
    if (names.length) return `container ${names.join(', ')} (published by docker)`
  } catch {
    // no docker answer: say what is known
  }
  return 'a process this user cannot see (another user\'s, or root\'s)'
}

/** Refuse to start when the app's port is already taken, naming who holds
 *  it — before a minute of infra and builds, and instead of vite's bare
 *  "Port N is already in use" (or, before --strictPort, a silent move to the
 *  next port). */
async function assertAppPortFree(ctx: Ctx, port: string, whence: string): Promise<void> {
  const holder = await portHolder(ctx, port)
  if (!holder) return
  ctx.log.die(
    `port ${port} (${whence}) is already taken by ${holder}. vite binds it strictly — moving to another ` +
      `port would leave this stack's URLs and its agents' gateway url on ${port}. Stop that process ` +
      '(another `talaria dev` for this checkout?) and run again.',
  )
}

export async function runDev(ctx: Ctx): Promise<number> {
  // The shim always runs the primary's CLI: from inside a worktree this would
  // bring up the PRIMARY's stack, so a run from another checkout stops here.
  refuseForeignCheckout(ctx, 'dev')
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

  // The port vite will bind, checked before anything else starts.
  const wtPort = envValue(uiEnv, 'TALARIA_WORKTREE') ? envValue(uiEnv, 'PORT') : undefined
  await assertAppPortFree(
    ctx,
    wtPort ?? VITE_SCRIPT_PORT,
    wtPort ? "this worktree's PORT in ui/.env" : "the dev script's port",
  )

  await warnDiskPressure(ctx)

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
    await ompAuthBridge(ctx, { quietWhenFresh: true })
    await rustApi(ctx, uiEnv)
    ctx.log.say(`app → http://127.0.0.1:${ctx.env.PORT ?? VITE_SCRIPT_PORT} (published to the host by the box compose)`)
    // Strict here too: the box compose publishes the container's 5273, so a
    // vite that moved to 5274 would serve nothing the host can reach.
    return ctx.run('bun', viteArgs(undefined), { cwd: join(ctx.root, 'ui') })
  }

  // A worktree (docs/WORKTREES.md) owns exactly two infra services — its own
  // postgres + redis, created by `talaria worktree` under project
  // talaria-wt-<name>. Aim every compose call and readiness probe at THEM:
  // with the default project/container names this would instead adopt (or
  // start!) the MAIN stack's containers from inside the worktree — the app
  // still read the worktree's DB via ui/.env, but "stop my stack" and the
  // health gates would have been reaching into main. Container names come from
  // the marker, ports from the URLs `talaria worktree` wrote.
  //
  // stack.ts names them, and refuses a shell var that names another stack's
  // container instead of letting it win. The same holds for every line a
  // worktree's ui/.env owns: apiEnv and vite both let the shell win, so a
  // stale DATABASE_URL, REDIS_URL or fleet project from another stack's shell
  // would put this worktree's servers on that stack. And a worktree with no
  // fleet project of its own renders its agents into main's: refused too.
  // The primary goes through the same door. Its shell is its override point,
  // but a shell that carries TALARIA_WORKTREE was started under a worktree's
  // `talaria dev` and holds that worktree's whole ui/.env (vite loads it into
  // process.env), and a `talaria-wt-*` fleet project is a worktree's: either
  // would start main's app and api on the worktree's database or fleet.
  const stack = devStack(ctx, uiEnv, 'dev')
  refuseWorktreeEnvOverride(ctx, stack, uiEnv, 'dev')
  if (stack.fleetProblem) {
    ctx.log.die(`${stack.fleetProblem}\n  \`talaria dev\` would start its api rendering agents there, so it refuses.`)
  }
  const worktree = stack.worktree
  if (worktree) {
    ctx.env.TALARIA_PG_CONTAINER = stack.pgContainer
    ctx.env.TALARIA_REDIS_CONTAINER = stack.redisContainer
    // The ports compose publishes come from the URLs. A shell port that is
    // not the URL's used to win (`??=`): the worktree's postgres then took,
    // say, main's 5544 while its own app dialled the URL's, and main's app,
    // on that port, would have reached the worktree's database.
    const pgPort = portOf(envValue(uiEnv, 'DATABASE_URL'))
    const redisPort = portOf(envValue(uiEnv, 'REDIS_URL'))
    for (const [key, port, url] of [['TALARIA_PG_PORT', pgPort, 'DATABASE_URL'], ['TALARIA_REDIS_PORT', redisPort, 'REDIS_URL']] as const) {
      const shell = ctx.env[key]
      if (port && shell !== undefined && shell !== port) {
        ctx.log.die(
          `this is worktree "${worktree}", whose ${url} in ui/.env names port ${port}, but ${key}=${shell} in the environment ` +
            `would publish its container on another. Refusing rather than guess: unset ${key}.`,
        )
      }
      if (port) ctx.env[key] = port
    }
    ctx.log.say(`infra (this worktree's postgres + redis — sidecars are the main stack's)`)
    // worktreeComposeFiles, not the dev pair: the stack `talaria worktree`
    // made runs on the default bridge (docker/worktree.compose.yml), and an
    // `up` without that file would recreate both containers on a project
    // network — the address-pool drain the file exists to stop.
    const code = await compose(ctx, { files: worktreeComposeFiles(ctx.root), project: `talaria-wt-${worktree}` }, ['up', '-d', 'postgres', 'redis'])
    if (code !== 0) ctx.log.die('worktree infra failed to start (run `bun talaria worktree` again? it was torn down?)')
    await releaseLegacyNetwork(ctx, worktree)
    return await waitThenRunApp(ctx, uiEnv, stack)
  }

  // The primary: the shell's TALARIA_*_CONTAINER is its override point, but
  // one that names a worktree's or a box's container (a shell under that
  // worktree's `talaria dev` inherits them) would make the `up` below
  // recreate main's postgres under that name, or remove it on the name
  // conflict. Checked before the `??=` lines, so only the shell's values are
  // judged.
  await refuseSideStackContainers(ctx, ALL_PRIMARY_CONTAINER_VARS, 'dev')

  // The MinIO-era spellings keep working: a ui/.env written before the engine
  // swap names the container and port under the old variables, and compose
  // now interpolates the new ones. Seed across so nobody's dev loop moves
  // underneath them; an explicit new value always wins.
  ctx.env.TALARIA_STORAGE_CONTAINER ??= ctx.env.TALARIA_MINIO_CONTAINER ?? envValue(uiEnv, 'TALARIA_MINIO_CONTAINER') ?? undefined
  ctx.env.TALARIA_STORAGE_PORT ??= ctx.env.TALARIA_MINIO_PORT ?? envValue(uiEnv, 'TALARIA_MINIO_PORT') ?? undefined

  // Built-in object storage creds: compose must match the app, so lift them
  // out of ui/.env for interpolation (both fall back to the same dev
  // defaults). Only if the shell hasn't already exported its own — the
  // environment is the override point, same as for the app.
  for (const varName of ['TALARIA_S3_ACCESS_KEY', 'TALARIA_S3_SECRET_KEY']) {
    const val = envValue(uiEnv, varName)
    if (val && !ctx.env[varName]) ctx.env[varName] = val
  }

  ctx.log.say('infra (postgres + redis + qdrant)')
  const devSpec = { files: stackComposeFiles(ctx.root, DEV_COMPOSE) }
  if ((await compose(ctx, devSpec, ['up', '-d', 'postgres', 'redis', 'qdrant'])) !== 0) {
    ctx.log.die('dev infra failed to start')
  }

  // Object storage started separately, and non-fatally — the rule embeddings
  // below already rides, which storage was the one sidecar still outside of. A single `up` resolves EVERY image before it creates ANY
  // container, so one unpullable image took postgres and redis down with it,
  // and `talaria dev` died at "dev infra failed to start" on a fresh clone
  // with the registry's error the only clue. That is exactly how the MinIO
  // withdrawal was felt, and the split is what keeps the next dead upstream
  // from being fatal. The app boots without object storage; uploads are what
  // degrade.
  ctx.log.say('object storage (versitygw)')
  if ((await compose(ctx, devSpec, ['up', '-d', 'storage'])) !== 0) {
    ctx.log.warn('object storage failed to start — attachments and uploads will fail until it is up.')
    ctx.log.warn(
      'If the image could not be pulled: it is ghcr.io/versity/versitygw, a public image — ' +
        'check network and daemon reachability rather than registry auth.',
    )
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

  return await waitThenRunApp(ctx, uiEnv, stack)
}

/** The network a worktree's postgres + redis sat on before
 *  docker/worktree.compose.yml moved them to the default bridge. */
export function legacyWorktreeNetwork(worktree: string): string {
  return `talaria-wt-${worktree}_default`
}

/** Give back the subnet of a worktree's old project network, now that the
 *  `up` before this has moved both its containers to the default bridge. The
 *  cleanup sweep leaves that network alone for as long as the worktree lives
 *  (its project still has containers), so on a host of live worktrees — the
 *  host whose address pools ran out — no subnet came back until each one was
 *  torn down. Best effort, and quiet unless it frees one: no such network is
 *  the usual case (every worktree cut since); one any container still names is
 *  kept, a stopped container too, since it could not start again without it;
 *  and a daemon too busy to answer changes nothing. */
async function releaseLegacyNetwork(ctx: Ctx, worktree: string): Promise<void> {
  const name = legacyWorktreeNetwork(worktree)
  const quick = { timeoutMs: 15_000 }
  try {
    await ctx.exec('docker', ['network', 'inspect', '--format', '{{.Id}}', name], quick)
  } catch {
    return
  }
  try {
    const ps = await ctx.exec('docker', ['ps', '-a', '--format', '{{.Networks}}'], quick)
    const named = ps.stdout.split('\n').some((line) => line.split(',').some((n) => n.trim() === name))
    if (named) return
    await ctx.exec('docker', ['network', 'rm', name], quick)
    ctx.log.ok(`removed ${name} — this worktree's postgres + redis run on the default bridge now`)
  } catch {
    // Still in use, or the daemon did not answer in time: the sweep takes it
    // once the worktree is torn down.
  }
}

/** The shared boot tail: infra readiness, deps, the fleet toolkit, then the
 *  vite process (this function's return IS dev's exit code). The probes exec
 *  into the containers devStack named: a worktree's own, or main's as
 *  compose reads the shell (an empty TALARIA_PG_CONTAINER is the default
 *  there, so `?? PG_CONTAINER` would have probed a container named ''). */
async function waitThenRunApp(ctx: Ctx, uiEnv: string, stack: DevStack): Promise<number> {
  ctx.log.say('waiting for postgres…')
  const pg = stack.pgContainer
  // A real query over TCP, not the socket pg_isready this was: on a fresh
  // volume that passed against the entrypoint's temporary init server, and
  // the app's migration pass then met "the database system is shutting down"
  // (compose.ts, pgReadyArgs).
  if (!(await waitForPostgres(ctx, pg, 40))) ctx.log.die('postgres never became ready')

  ctx.log.say('waiting for redis…')
  const redis = stack.redisContainer
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
  await ompAuthBridge(ctx)
  await rustApi(ctx, uiEnv)

  // A worktree's ui/.env carries its own PORT (the allocated slot), and
  // vite's dev script hardcodes 5273, so the worktree's is forwarded — and
  // bound strictly (viteArgs says why).
  const wtPort = envValue(uiEnv, 'TALARIA_WORKTREE') ? envValue(uiEnv, 'PORT') : undefined

  ctx.log.say(`app → http://localhost:${wtPort ?? VITE_SCRIPT_PORT}`)
  return ctx.run('bun', viteArgs(wtPort), { cwd: join(ctx.root, 'ui') })
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
