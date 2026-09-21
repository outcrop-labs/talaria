#!/usr/bin/env node
// deploy-smoke — boot a real Talaria instance from an image and ask the
// questions a deploy has to answer.
//
// WHY THIS EXISTS. Nothing else in this repository runs the artifact. CI builds
// the bundles and runs the unit suites; `chassis-boot-smoke.mjs` boots an AGENT
// container, not the app. The 2026-09-17 incident is the shape of the gap: the
// build was green, the tests were green, and every page on every instance
// answered 404 because a built bundle moved one directory and nothing had ever
// served it. This script serves it.
//
// WHAT IT BOOTS. postgres:16-alpine and redis:7-alpine (the only two
// dependencies the container's own entrypoint gates on) and the image under
// test, as a FRESH INSTANCE: a first-boot state dir, so the entrypoint
// generates its own secrets, and an empty database, so the boot migration pass
// runs for real instead of finding a schema already there. That is the deploy
// an operator does on a new host — the one with the least between it and a
// broken build.
//
// WHAT IT ASSERTS, and the failure each one catches:
//
//   1. /api/healthz reports ok — postgres and redis round-trip, the hop to the
//      Rust api works, and (when the boot pass reports on itself) migrations
//      succeeded. The app flips this to 503 when the migration pass failed, so
//      a green-vs-broken instance cannot pass here.
//   2. The reported version is the commit that was built (`--expect-version`).
//      A deploy that cannot say which commit it is standing on proves nothing
//      about the commit being deployed.
//   3. GET / serves the SPA shell — text/html, real HTML. This is the
//      2026-09-17 assertion, from outside the container.
//   4. GET /home/inbox serves it too — the client-route fallback, which is a
//      different code path from the bare root.
//   5. GET /api/well-known/talaria-instance returns the Rust api's JSON. A
//      public route, so it needs no session: it proves the proxy, the spawn and
//      the handler chain, not just that something is listening.
//   6. The container is still up when all of that is true. A crash-loop that
//      happened to answer a probe between restarts is not a deploy.
//
// WHAT IT DELIBERATELY DOES NOT DO. No sidecars (qdrant, embeddings, minio,
// searxng), no fleet, no LLM: a first boot has none of them and must still come
// up. It does not push anything, so it is safe to run against any image, and it
// never reuses a stack — the project name is fixed, torn down before it starts
// and after it finishes, so a local run cannot leave anything behind.
//
// RUN IT BY HAND:
//   docker build -t talaria-rc:local .
//   node scripts/deploy-smoke.mjs --image talaria-rc:local --keep   # keeps the stack, to look around
//
// rc-deploy.yml runs exactly this against the image it builds for rc's tip.

import { execFile } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

const exec = promisify(execFile)

const PROJECT = 'talaria-deploy-smoke'
const SERVICE = 'talaria'
const CONTAINER = `${PROJECT}-${SERVICE}-1`
/** The entrypoint waits up to 120s for postgres and redis, then the boot
 *  migration pass runs before listen(). 300s is generous enough that a failure
 *  here is the image's, not the runner's. */
const BOOT_DEADLINE_MS = 300_000
const BOOT_POLL_MS = 3_000
/** Per-request. The app answers these from memory; slow is a failure. */
const REQUEST_TIMEOUT_MS = 10_000

const log = (msg) => console.log(`[deploy-smoke] ${msg}`)

class SmokeFailure extends Error {}

const fail = (msg) => {
  throw new SmokeFailure(msg)
}

// ── arguments ───────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const opts = { image: '', expectVersion: '', port: 0, keep: false }
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]
    if (arg === '--image') opts.image = argv[++i] ?? ''
    else if (arg === '--expect-version') opts.expectVersion = argv[++i] ?? ''
    else if (arg === '--port') opts.port = Number(argv[++i] ?? 0)
    else if (arg === '--keep') opts.keep = true
    else if (arg === '-h' || arg === '--help') {
      console.log('usage: node scripts/deploy-smoke.mjs --image <ref> [--expect-version <string>] [--port <n>] [--keep]')
      process.exit(0)
    } else fail(`unknown argument \`${arg}\``)
  }
  if (!opts.image) fail('--image is required (the image under test, e.g. talaria-rc:abc123def456)')
  return opts
}

/** A port nothing is listening on, so the smoke cannot collide with a dev
 *  stack on the machine running it. */
function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer()
    server.unref()
    server.on('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const address = server.address()
      const port = typeof address === 'object' && address ? address.port : 0
      server.close(() => (port ? resolve(port) : reject(new Error('no port'))))
    })
  })
}

// ── docker ──────────────────────────────────────────────────────────────────

async function docker(args, extra = {}) {
  return exec('docker', args, { timeout: 300_000, maxBuffer: 8 << 20, ...extra })
}

function composeFile(port, image) {
  return `# GENERATED by scripts/deploy-smoke.mjs — a throwaway instance under test.
name: ${PROJECT}

services:
  postgres:
    image: docker.io/library/postgres:16-alpine
    # No restart policy anywhere in this file (same rule as the chassis smoke):
    # a crash-loop must look like a failure, not like a slow start.
    restart: 'no'
    environment:
      POSTGRES_USER: talaria
      POSTGRES_PASSWORD: talaria
      POSTGRES_DB: talaria
    healthcheck:
      test: ['CMD-SHELL', 'pg_isready -U talaria -d talaria']
      interval: 3s
      timeout: 3s
      retries: 20

  redis:
    image: docker.io/library/redis:7-alpine
    restart: 'no'
    healthcheck:
      test: ['CMD', 'redis-cli', 'ping']
      interval: 3s
      timeout: 3s
      retries: 20

  ${SERVICE}:
    image: ${image}
    restart: 'no'
    # Loopback only: nothing outside this machine should be able to reach a
    # smoke instance, which is running on generated secrets.
    ports:
      - '127.0.0.1:${port}:5273'
    environment:
      # The image carries no config — the environment is the only channel
      # (docker/entrypoint.sh). These are the two dependencies it gates on;
      # the entrypoint generates the secrets into the fresh state dir below.
      DATABASE_URL: postgres://talaria:talaria@postgres:5432/talaria
      REDIS_URL: redis://redis:6379
      TALARIA_STATE_DIR: /var/lib/talaria
    volumes:
      # A named volume, not a host bind: a fresh one is populated from the
      # image's own /var/lib/talaria (already owned by the app user), so this is
      # a first boot on an empty state dir with no host plumbing.
      - talaria-state:/var/lib/talaria
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy
    healthcheck:
      # The same probe the deployment compose file uses.
      test: ['CMD-SHELL', 'wget -qO- http://127.0.0.1:5273/api/healthz >/dev/null 2>&1 || exit 1']
      interval: 5s
      timeout: 5s
      retries: 30
      start_period: 90s

volumes:
  talaria-state:
`
}

// ── the instance ────────────────────────────────────────────────────────────

const base = (port) => `http://127.0.0.1:${port}`

async function get(port, path) {
  const res = await fetch(`${base(port)}${path}`, {
    signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    redirect: 'manual',
  })
  const body = await res.text()
  return { status: res.status, type: res.headers.get('content-type') ?? '', body }
}

/** Container state, so a dead container fails as "it exited", not as a
 *  five-minute timeout. */
async function containerState() {
  try {
    // RestartCount is a top-level field, NOT under .State — asking for
    // .State.RestartCount fails the whole inspect, which is how this line
    // first reported a live container as "absent".
    const { stdout } = await docker(['inspect', '-f', '{{.State.Status}} {{.RestartCount}}', CONTAINER])
    const [status, restarts] = stdout.trim().split(/\s+/)
    return { status, restarts: Number(restarts ?? 0) }
  } catch {
    return { status: 'absent', restarts: 0 }
  }
}

async function containerLogs(lines = 120) {
  try {
    const { stdout, stderr } = await docker(['logs', '--tail', String(lines), CONTAINER])
    return `${stdout}${stderr}`
  } catch (e) {
    return `(could not read container logs: ${e instanceof Error ? e.message : String(e)})`
  }
}

/** Wait for a health answer, watching for the container dying underneath it. */
async function waitForHealth(port) {
  const deadline = Date.now() + BOOT_DEADLINE_MS
  let last = 'no answer yet'
  while (Date.now() < deadline) {
    const state = await containerState()
    if (state.status === 'exited' || state.status === 'dead') {
      fail(`the container ${state.status} after ${state.restarts} restart(s) — the image does not boot`)
    }
    try {
      const res = await get(port, '/api/healthz')
      if (res.status === 200) return JSON.parse(res.body)
      last = `healthz answered ${res.status}: ${res.body.slice(0, 200)}`
      // 503 is the app saying it is up but not ok. That is an answer, not a
      // slow boot: report it with its own body rather than waiting it out.
      if (res.status === 503) fail(`healthz reports the instance is not ok — ${res.body.slice(0, 400)}`)
    } catch (e) {
      last = e instanceof Error ? e.message : String(e)
    }
    await new Promise((r) => setTimeout(r, BOOT_POLL_MS))
  }
  fail(`no healthy instance within ${BOOT_DEADLINE_MS / 1000}s (last: ${last})`)
}

// ── assertions ──────────────────────────────────────────────────────────────

function assertHealth(health, expectVersion) {
  log(`healthz: ${JSON.stringify(health)}`)
  if (health.status !== 'ok') fail(`healthz status is "${health.status}", expected "ok"`)

  const checks = health.checks ?? {}
  for (const name of ['postgres', 'redis']) {
    if (!checks[name]?.ok) fail(`healthz check \`${name}\` is not ok: ${JSON.stringify(checks[name])}`)
  }
  // The hop to the Rust api. Present whenever this process fronts one — which
  // is what the image does (TALARIA_API_BIN, Dockerfile). A missing check here
  // means the api is not wired at all, which no other assertion would notice
  // until a real /api request 502s in production.
  if (!checks.rustApi) fail('healthz reports no `rustApi` check — this image fronts no Rust api')
  if (!checks.rustApi.ok) fail(`the hop to the Rust api is not ok: ${JSON.stringify(checks.rustApi)}`)
  if (checks.migrations && !checks.migrations.ok) {
    fail(`the boot migration pass did not succeed: ${JSON.stringify(checks.migrations)}`)
  }

  if (expectVersion) {
    if (health.version !== expectVersion) {
      fail(`the instance reports version ${JSON.stringify(health.version)}, expected ${JSON.stringify(expectVersion)} — the image is not the commit under test`)
    }
    log(`version: ${health.version} (as built)`)
  } else if (!health.version) {
    log('version: null — a locally built image with no VERSION build-arg')
  }
}

async function assertSpaShell(port) {
  for (const path of ['/', '/home/inbox']) {
    const res = await get(port, path)
    if (res.status !== 200) fail(`GET ${path} answered ${res.status}, expected 200`)
    if (!res.type.startsWith('text/html')) fail(`GET ${path} served content-type "${res.type}", expected text/html`)
    if (!/<html/i.test(res.body)) fail(`GET ${path} served a body that is not HTML: "${res.body.slice(0, 80)}"`)
    log(`GET ${path} → 200 text/html (${res.body.length} bytes)`)
  }
}

async function assertRustApi(port) {
  const res = await get(port, '/api/well-known/talaria-instance')
  if (res.status !== 200) fail(`GET /api/well-known/talaria-instance answered ${res.status}, expected 200 — the Rust api is not answering behind the proxy`)
  let payload
  try {
    payload = JSON.parse(res.body)
  } catch {
    fail(`the api route answered non-JSON: "${res.body.slice(0, 120)}"`)
  }
  if (!payload.instance) fail(`the api route answered without an \`instance\`: ${res.body.slice(0, 200)}`)
  log(`GET /api/well-known/talaria-instance → 200 ${JSON.stringify(payload).slice(0, 120)}`)
}

// ── main ────────────────────────────────────────────────────────────────────

async function main() {
  const opts = parseArgs(process.argv.slice(2))
  const port = opts.port || (await freePort())
  const dir = mkdtempSync(join(tmpdir(), 'talaria-deploy-smoke-'))
  const file = join(dir, 'compose.yml')
  writeFileSync(file, composeFile(port, opts.image))
  const compose = (args, extra) => docker(['compose', '-p', PROJECT, '-f', file, ...args], extra)

  log(`image: ${opts.image}`)
  log(`project: ${PROJECT} (port 127.0.0.1:${port}, state volume fresh)`)

  let failure = null
  try {
    // A stack left by a previous run would be answering on this project name.
    await compose(['down', '-v', '--remove-orphans']).catch(() => {})

    log('starting postgres, redis, the app…')
    await compose(['up', '-d'], { timeout: 600_000 })

    const health = await waitForHealth(port)
    assertHealth(health, opts.expectVersion)
    await assertSpaShell(port)
    await assertRustApi(port)

    const state = await containerState()
    if (state.status !== 'running' || state.restarts !== 0) {
      fail(`the container is "${state.status}" with ${state.restarts} restart(s) after answering — that is a crash-loop, not a deploy`)
    }
    log('the instance is up, serving, and has not restarted')

    console.log(
      `\n[deploy-smoke] PASS — ${opts.image} booted as a fresh instance: ` +
        `healthz ok, SPA shell serving, Rust api answering${opts.expectVersion ? `, version ${opts.expectVersion}` : ''}.\n`
    )
  } catch (e) {
    failure = e
    console.error(`\n[deploy-smoke] FAIL — ${e instanceof Error ? e.message : String(e)}\n`)
    if (!(e instanceof SmokeFailure)) {
      // An unexpected error (docker itself, a bad argument): its own message is
      // the only useful thing, and the container logs below may still explain it.
      console.error(e)
    }
    try {
      const { stdout } = await compose(['ps'])
      console.error(`--- compose ps ---\n${stdout}`)
    } catch {
      /* not up, or docker is gone */
    }
    console.error(`--- ${CONTAINER} logs (last 120) ---\n${await containerLogs()}`)
  } finally {
    if (opts.keep) log(`--keep: the stack is still up. Inspect, then: docker compose -p ${PROJECT} -f ${file} down -v`)
    else await compose(['down', '-v', '--remove-orphans']).catch(() => {})
    rmSync(dir, { recursive: true, force: true })
  }

  process.exit(failure ? 1 : 0)
}

await main()