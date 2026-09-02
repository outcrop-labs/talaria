// Production server: a Node HTTP server wrapping the app's fetch handler
// (dist/server/server.js) and serving the client assets (dist/client). Keeps the
// streaming pump so SSE chat responses flush incrementally.
import { createServer } from 'node:http'
import { spawn } from 'node:child_process'
import { readFile, stat } from 'node:fs/promises'
import { readFileSync, existsSync } from 'node:fs'
import { join, extname } from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = fileURLToPath(new URL('.', import.meta.url))

// ── ui/.env, before anything reads process.env ───────────────────────────────
// `vite dev` loads this file; `node server-entry.js` did not. Same install,
// two different views of the environment — and because setup.sh generates
// AUTH_SECRET and TALARIA_SECRET_KEY as two SEPARATE random values, and
// secretbox falls back from one to the other, that difference silently changed
// the key every stored secret is wrapped with. A database created under `dev`
// then served by `npm start` could not decrypt its own provider keys, with
// nothing in the config having changed. Loading it here makes the two modes
// agree.
//
// REAL ENVIRONMENT WINS. A value already in process.env — systemd, docker,
// Kubernetes, the shell — is never overwritten by the file. This is a fallback
// for the single-box case, not a source of truth that can surprise a deploy.
function loadEnvFile() {
  const path = join(__dirname, '.env')
  if (!existsSync(path)) return 0
  let loaded = 0
  for (const raw of readFileSync(path, 'utf8').split('\n')) {
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue
    const eq = line.indexOf('=')
    if (eq < 1) continue
    const key = line.slice(0, eq).trim()
    if (key in process.env) continue // already set: the real environment wins
    let value = line.slice(eq + 1).trim()
    // Strip one layer of matching quotes, the way every .env reader does.
    if (value.length >= 2 && ((value[0] === '"' && value.at(-1) === '"') || (value[0] === "'" && value.at(-1) === "'"))) {
      value = value.slice(1, -1)
    }
    process.env[key] = value
    loaded++
  }
  return loaded
}
const envLoaded = loadEnvFile()
if (envLoaded) console.log(`[talaria-ui] loaded ${envLoaded} value(s) from ui/.env (existing environment left untouched)`)

// Mark the runtime BEFORE the app graph loads: the in-app updater only runs
// on a real server install, and this is how it tells one from `vite dev`
// (which reloads on file change and must never pull the repo mid-session).
process.env.TALARIA_RUNTIME = 'prod-server'

// Fail the boot on a bad environment rather than the first request that touches
// a missing value. env.ts is TypeScript, which Node executes directly from
// v22.18 (unflagged type stripping) — on an older runtime we say so and carry
// on rather than refusing to start over a check we couldn't run.
let validateEnv = null
try {
  ;({ validateEnv } = await import('./src/server/env.ts'))
} catch (err) {
  console.warn(
    `[talaria-ui] WARNING: skipping environment validation — this Node (${process.version}) cannot load src/server/env.ts. ` +
      `Upgrade to Node >= 22.18 to get it. (${err instanceof Error ? err.message : String(err)})`,
  )
}
if (validateEnv) {
  try {
    validateEnv()
  } catch (err) {
    console.error(`[talaria-ui] ${err instanceof Error ? err.message : String(err)}`)
    process.exit(1)
  }
}

// Imported AFTER the env is in place: a static import is hoisted above every
// statement here, and the server graph reads process.env as it loads.
const { default: server, migrate, writeHeadHeaders } = await import('./dist/server/server.js')

// The boundary conversion for res.writeHead. A bundle built before
// writeHeadHeaders existed exports none — same stale-dist case as `migrate`
// below; the old one-liner conversion is kept as the fallback so a mismatched
// dist degrades to the old behavior instead of 500ing every request, with
// this line saying which box needs a rebuild.
const headersForWriteHead =
  typeof writeHeadHeaders === 'function'
    ? writeHeadHeaders
    : (response) => {
        console.warn('[talaria-ui] dist bundle predates writeHeadHeaders — rebuild to fix multi-cookie responses (OAuth login)')
        return Object.fromEntries(response.headers.entries())
      }

const CLIENT_DIR = join(__dirname, 'dist', 'client')
const port = parseInt(process.env.PORT || '3000', 10)
const host = process.env.HOST || '0.0.0.0'

const MIME_TYPES = {
  '.js': 'application/javascript',
  '.mjs': 'application/javascript',
  '.css': 'text/css',
  '.html': 'text/html',
  '.json': 'application/json',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.map': 'application/json',
  '.webmanifest': 'application/manifest+json',
}

async function tryServeStatic(req, res) {
  const url = new URL(req.url || '/', `http://${req.headers.host || 'localhost'}`)
  const pathname = decodeURIComponent(url.pathname)
  if (pathname.includes('..')) return false

  const filePath = join(CLIENT_DIR, pathname)
  if (!filePath.startsWith(CLIENT_DIR)) return false

  // Hashed asset requests must 404 (not fall through to the HTML shell) so stale
  // chunks after a deploy fail cleanly instead of rendering a blank SPA.
  const isAsset = pathname.startsWith('/assets/')
  try {
    const fileStat = await stat(filePath)
    if (!fileStat.isFile()) throw new Error('not a file')
    const ext = extname(filePath).toLowerCase()
    const data = await readFile(filePath)
    const headers = { 'Content-Type': MIME_TYPES[ext] || 'application/octet-stream', 'Content-Length': data.length }
    if (isAsset) headers['Cache-Control'] = 'public, max-age=31536000, immutable'
    res.writeHead(200, headers)
    res.end(data)
    return true
  } catch {
    if (isAsset) {
      res.writeHead(404, { 'Content-Type': 'text/plain', 'Cache-Control': 'no-store' })
      res.end('Asset not found')
      return true
    }
    return false
  }
}

// A query string is NOT safe to log verbatim. Several routes carry live
// credentials in it: the OAuth callbacks (/api/auth/google/callback,
// /api/integrations/google/callback, .../google/org/callback,
// /api/mcp/oauth/callback) take `?code=` — a redeemable authorization code —
// plus the `?state=` CSRF token, and /api/join takes `?token=`, a redeemable
// invite. Those are exactly the routes whose 500s someone goes and reads, so
// logging the raw search string hands a log reader working secrets at the one
// moment they are most likely to look.
//
// Redact by key rather than dropping the query entirely: `?since=…&kind=…` is
// often the whole diagnosis, and a log you can't diagnose from is why this
// logging was added in the first place.
const REDACTED = '[redacted]'

// Matched against the key lowercased with separators stripped, so `access_token`,
// `access-token` and `accessToken` all collapse to `accesstoken`.
const SENSITIVE_KEYS = new Set([
  'code',
  'state',
  'token',
  'key',
  'secret',
  'password',
  'passwd',
  'pwd',
  'auth',
  'authorization',
  'credential',
  'credentials',
  'signature',
  'sig',
  'assertion',
  'session',
  'sessionid',
  'sid',
  'jwt',
  'otp',
  'nonce',
  'ticket',
])
// Substring matches, for the compound names a fixed list can't enumerate:
// access_token / refresh_token / id_token / client_secret / api_key / …
const SENSITIVE_FRAGMENTS = ['token', 'secret', 'password', 'apikey', 'privatekey', 'signature', 'credential']

// A key the list above doesn't know about, holding a long opaque value, is far
// more likely to be a credential added after this file was last touched than a
// useful diagnostic. Report its length instead of trusting it.
const MAX_LOGGED_VALUE = 64

function isSensitiveParam(rawKey) {
  const key = rawKey.toLowerCase().replace(/[^a-z0-9]/g, '')
  if (SENSITIVE_KEYS.has(key)) return true
  return SENSITIVE_FRAGMENTS.some((fragment) => key.includes(fragment))
}

/** `?code=4/0Ab…&state=xyz&next=/boards` -> `?code=[redacted]&state=[redacted]&next=%2Fboards` */
export function redactQueryForLog(search) {
  if (!search || search === '?') return ''
  const parts = []
  for (const [rawKey, rawValue] of new URLSearchParams(search)) {
    const key = encodeURIComponent(rawKey)
    if (rawValue === '') {
      // Nothing to leak, and `code=[redacted]` would wrongly imply one was sent.
      parts.push(`${key}=`)
    } else if (isSensitiveParam(rawKey)) {
      parts.push(`${key}=${REDACTED}`)
    } else if (rawValue.length > MAX_LOGGED_VALUE) {
      parts.push(`${key}=[redacted:${rawValue.length}b]`)
    } else {
      // Re-encoded so a crafted value can't forge a newline and fake a log line.
      parts.push(`${key}=${encodeURIComponent(rawValue)}`)
    }
  }
  return parts.length ? `?${parts.join('&')}` : ''
}

// Enough identity to correlate a 500 with a user report without logging any
// credential. The session cookie's VALUE is a bearer token, so only its
// presence is recorded — "was this an anonymous request or a signed-in one".
let requestSeq = 0
function requestContext(req, url) {
  const cookies = req.headers.cookie || ''
  return {
    id: `${Date.now().toString(36)}-${(requestSeq = (requestSeq + 1) % 0xffffff).toString(36)}`,
    method: req.method || 'GET',
    path: url.pathname + redactQueryForLog(url.search),
    // x-forwarded-for is client-controlled; it is a hint for correlation only.
    ip: (req.headers['x-forwarded-for'] || '').split(',')[0].trim() || req.socket.remoteAddress || '-',
    auth: cookies.includes('talaria_session=') ? 'session' : 'anon',
    started: Date.now(),
  }
}

// Errors that mean "the peer went away", not "this server is broken".
const DISCONNECT_CODES = new Set([
  'ECONNRESET',
  'EPIPE',
  'ECANCELED',
  'ABORT_ERR',
  'ERR_STREAM_PREMATURE_CLOSE',
  'ERR_STREAM_DESTROYED',
  'ERR_STREAM_WRITE_AFTER_END',
  'UND_ERR_ABORTED',
])

/** First abort-family code in the error's cause chain, or null. */
function disconnectCode(err) {
  for (let e = err, depth = 0; e && typeof e === 'object' && depth < 5; e = e.cause, depth += 1) {
    if (typeof e.code === 'string' && DISCONNECT_CODES.has(e.code)) return e.code
    if (e.name === 'AbortError') return 'ABORT_ERR'
  }
  return null
}

// SSE is this app's primary workload and chat-view deliberately breaks out of
// streams early, so a client hanging up mid-stream is routine — it must not be
// dressed up as `500 GET /api/…` with a stack, or the constant noise buries the
// signal this logging exists to provide. The socket has to actually be gone
// before we downgrade, so an ECONNRESET from an *upstream* call (LLM provider,
// MCP server) still gets logged as the server error it is.
function clientDisconnect(err, req, res) {
  const code = disconnectCode(err)
  if (!code) return null
  const socketGone = res.destroyed || res.writableEnded || !res.writable || req.destroyed || req.aborted
  return socketGone ? code : null
}

function logClientDisconnect(ctx, code, isEventStream) {
  // One line, no stack: routine, and only interesting in aggregate.
  console.warn(
    `[talaria-ui] client disconnect ${ctx.method} ${ctx.path} ${Date.now() - ctx.started}ms` +
      ` id=${ctx.id} auth=${ctx.auth} sse=${isEventStream} code=${code}`,
  )
}

function logRequestError(ctx, err, note) {
  console.error(
    `[talaria-ui] 500 ${ctx.method} ${ctx.path} ${Date.now() - ctx.started}ms` +
      ` id=${ctx.id} ip=${ctx.ip} auth=${ctx.auth}${note ? ` ${note}` : ''}`,
    err,
  )
}

/** Resolve when the socket can take more bytes — or when it dies, so a stalled
 *  client can't leave this handler awaiting a 'drain' that never comes. */
function waitForDrain(res) {
  return new Promise((resolve) => {
    const done = () => {
      res.off('drain', done)
      res.off('close', done)
      res.off('error', done)
      resolve()
    }
    res.once('drain', done)
    res.once('close', done)
    res.once('error', done)
  })
}

async function requestHandler(req, res) {
  if (req.method === 'GET' || req.method === 'HEAD') {
    if (await tryServeStatic(req, res)) return
  }

  const url = new URL(req.url || '/', `http://${req.headers.host || 'localhost'}`)
  const headers = new Headers()
  for (const [k, v] of Object.entries(req.headers)) {
    if (v) headers.set(k, Array.isArray(v) ? v.join(', ') : v)
  }

  let body = null
  if (req.method !== 'GET' && req.method !== 'HEAD') {
    body = await new Promise((resolve) => {
      const chunks = []
      req.on('data', (c) => chunks.push(c))
      req.on('end', () => resolve(Buffer.concat(chunks)))
    })
  }

  const request = new Request(url.toString(), { method: req.method, headers, body, duplex: 'half' })
  const ctx = requestContext(req, url)
  let isEventStream = false
  let reader = null
  try {
    const response = await server.fetch(request)
    isEventStream = (response.headers.get('content-type') || '').includes('text/event-stream')
    res.writeHead(response.status, headersForWriteHead(response))
    if (response.body) {
      reader = response.body.getReader()
      for (;;) {
        const { done, value } = await reader.read()
        if (done) break
        // The client hung up (closed tab, dropped SSE). Stop pulling from the
        // upstream stream instead of writing into a dead socket.
        if (res.destroyed || res.writableEnded) {
          await reader.cancel().catch(() => {})
          return
        }
        // Respect backpressure: a slow SSE consumer used to buffer unboundedly
        // in this process because the return value of write() was ignored.
        if (!res.write(value)) await waitForDrain(res)
      }
      res.end()
    } else {
      res.end(await response.text())
    }
  } catch (err) {
    const disconnected = clientDisconnect(err, req, res)
    if (disconnected) logClientDisconnect(ctx, disconnected, isEventStream)
    else logRequestError(ctx, err, `sent=${res.headersSent} sse=${isEventStream}`)
    if (reader) await reader.cancel().catch(() => {})
    if (res.writableEnded || res.destroyed) return

    if (!res.headersSent) {
      // Nothing has gone out yet: a clean 500 is still possible.
      res.writeHead(500, { 'Content-Type': 'text/plain', 'Cache-Control': 'no-store' })
      res.end('Internal Server Error')
      return
    }

    // Headers (and probably some body) already went out. The old code called
    // res.end('Internal Server Error') here regardless, which INJECTED those
    // literal bytes into whatever was mid-flight — for SSE, this app's primary
    // workload, straight into the event stream, where the client parses them as
    // a malformed frame (or worse, a truncated JSON payload).
    if (isEventStream) {
      // Say it in the stream's own grammar: a well-formed terminal error event,
      // then close so EventSource reconnects. `sse-parse.ts` ignores the frame
      // (no `choices`/`usage`), and an EventSource dispatches it as an "error"
      // event — the same handler a transport failure hits, which is where a
      // stream that died mid-flight belongs.
      try {
        res.write('event: error\ndata: {"error":"stream_failed"}\n\n')
      } catch {
        /* socket already gone */
      }
      res.end()
    } else {
      // Any other partially written body (HTML shell, JSON, a download): abort
      // the connection. A truncated transfer is an honest failure signal;
      // appending prose to half a document is corruption.
      res.destroy()
    }
  }
}

// A rejected promise nobody awaited used to take the whole cockpit down with
// Node's default handling, and nothing outside the process could tell why. Log
// it loudly — with a stack — and keep serving: one orphaned promise is not
// evidence that the server's state is corrupt, and dropping every open SSE
// stream is a far worse outcome than a noisy line in the log.
// (`uncaughtException` is deliberately NOT handled: there the state genuinely
// is unknown, and Node's default — print the stack, exit non-zero, let the
// supervisor restart us — is the behaviour we want.)
process.on('unhandledRejection', (reason) => {
  const detail = reason instanceof Error ? reason.stack || reason.message : reason
  console.error('[talaria-ui] UNHANDLED REJECTION (process kept alive, this is a bug):', detail)
})

// ── Database migrations ───────────────────────────────────────────────────────
//
// The pass used to be purely lazy: it fired on the first table-backed db()
// call, and before the api cutover that was enough — the TS server served the
// tables, so the first page hit migrated a fresh database. Post-cutover every
// table query is the Rust api's (which owns no DDL by design), and this
// process's boot path touches no table at all — healthz is connectivity-only.
// A fresh database therefore never migrated: the container came up "healthy"
// with zero tables, /claim 500'd, and every scheduler tick logged
// `relation "..." does not exist`. Boot runs the pass itself instead, BEFORE
// the api spawns (its scheduler reads tables from its first tick) and before
// listen().
//
// Bounded and non-fatal, the same rule as the boot probe below: a box whose
// database is down at boot must still reach listen() and say so through
// /api/healthz, not hang here. Past the deadline the pass keeps running — db()
// callers await the same cached promise — and a hard failure logs the reason
// and lets every table query 500 with the log above saying why.
const MIGRATION_BOOT_TIMEOUT_MS = 60_000

async function runBootMigrations() {
  if (typeof migrate !== 'function') {
    // A bundle built before this export existed: behave like the old world.
    console.warn('[talaria-ui] server bundle exports no migrate() — schema setup waits for the first table query')
    return
  }
  const started = Date.now()
  let bell
  const deadline = new Promise((resolve) => {
    bell = setTimeout(() => resolve(null), MIGRATION_BOOT_TIMEOUT_MS)
  })
  try {
    const result = await Promise.race([migrate(), deadline])
    if (result === null) {
      console.warn(
        `[talaria-ui] migrations still running after ${MIGRATION_BOOT_TIMEOUT_MS / 1000}s — continuing boot; ` +
          'table queries wait for the pass to finish',
      )
    } else if (result.applied === 0) {
      console.log(`[talaria-ui] migrations → schema already current (${result.total} statements, ${Date.now() - started}ms)`)
    } else {
      console.log(`[talaria-ui] migrations → applied ${result.applied} statement(s) (${result.total} total, ${Date.now() - started}ms)`)
    }
  } catch (err) {
    // Record the failure for /api/healthz: the app still serves (the rule in
    // the header above), but a probe must be able to tell "boot fine" from
    // "schema broken" — this is what flips the compose healthcheck (and any
    // deploy gate watching it) instead of a green container that 500s every
    // table query. A safe short code and a timestamp only, healthz's own law;
    // the full error is logged on the line below.
    globalThis.__talariaBootMigrationError = {
      code:
        typeof err?.code === 'string' && /^[A-Z0-9_]{1,20}$/.test(err.code) ? err.code : 'MIGRATION_FAILED',
      at: Date.now(),
    }
    console.error(
      '[talaria-ui] migrations FAILED — the schema is not in place and every table query will 500 until this is fixed:',
      err,
    )
  } finally {
    clearTimeout(bell)
  }
}

await runBootMigrations()

// ── The Rust api ─────────────────────────────────────────────────────────────
//
// Every /api/* request this server receives is the Rust api's to answer (the
// four TS residents aside — src/server/rust-proxy.ts is the boundary), and
// the background-job schedule is its too: the TS scheduler handshake this
// entry used to run left with the cutover, because the TS runtime that could
// have armed it is gone. What stays here is the process shape: production
// runs one supervisor and one log, so this process owns the api binary as a
// child — the same contract `talaria dev` gives vite in development.
//
// · ADOPT before spawn: something already answering on the port is the api —
//   a separately supervised instance, or a second `bun run start` on the same
//   box. A second binary would just lose the race for the port and die
//   loudly.
// · The hop is wired as a side effect: TALARIA_RUST_API_URL defaults to the
//   loopback URL the child binds, so the proxy is armed the moment the api
//   is. (An operator who set the env explicitly keeps their value — it is
//   where the URL came from.)
// · Non-fatal on purpose: a missing binary must not stop the SPA from
//   listening. The proxy answers 502 with its fixed sentence, /api/healthz
//   names the unreachable check, and the lines below say it at volume.
//   TALARIA_API=off is the escape hatch for a box that supervises the api
//   entirely on its own — it disables spawning, not adoption.
const API_PORT = Number(process.env.TALARIA_API_PORT || 5274)
const API_URL = process.env.TALARIA_RUST_API_URL || `http://127.0.0.1:${API_PORT}`

let apiChild = null
let shuttingDown = false

async function apiUp(timeoutMs) {
  try {
    await fetch(`${API_URL}/api/healthz`, { signal: AbortSignal.timeout(timeoutMs) })
    return true // any answer — healthy or its own 503 — means the api is THERE
  } catch {
    return false
  }
}

async function ensureRustApi() {
  // `off` is the whole-env contract, not just the proxy's: a process whose
  // hop is stood down must not go looking for (or spawning) an api either.
  if (process.env.TALARIA_RUST_API_URL === 'off') {
    console.warn('[talaria-ui] TALARIA_RUST_API_URL=off — the hop is stood down; not probing or spawning the Rust api.')
    return
  }
  if (await apiUp(1500)) {
    process.env.TALARIA_RUST_API_URL = API_URL
    console.log(`[talaria-ui] rust api → already listening on :${API_PORT}; adopting it`)
    return
  }
  if (process.env.TALARIA_API === 'off') {
    console.warn(
      `[talaria-ui] TALARIA_API=off and nothing answered on :${API_PORT} — not spawning the Rust api.` +
        ' Every /api/* request fails until one is running there.',
    )
    return
  }
  const bin = process.env.TALARIA_API_BIN || join(__dirname, '../api/target/release/talaria-api')
  if (!existsSync(bin)) {
    console.error(
      `[talaria-ui] no Rust api binary at ${bin} (build it, or set TALARIA_API_BIN) —` +
        ' every /api/* request will 502. The SPA will serve; the api is the product surface.',
    )
    return
  }
  apiChild = spawn(bin, [], {
    env: { ...process.env, TALARIA_API_PORT: String(API_PORT) },
    stdio: 'inherit',
  })
  process.env.TALARIA_RUST_API_URL = API_URL
  apiChild.on('error', (err) => console.error('[talaria-ui] rust api spawn failed:', err))
  apiChild.on('close', (code) => {
    if (shuttingDown) return
    console.error(
      `[talaria-ui] RUST API EXITED (code ${code}) — every /api/* request fails until it is back.` +
        ' Exiting so the supervisor restarts the pair together.',
    )
    process.exit(1)
  })
  console.log(`[talaria-ui] rust api → ${bin} on :${API_PORT}`)
  // Wait briefly for the bind: the boot probe below reports the api's
  // reachability as of boot, and a binary that is merely still execing should
  // not read as an outage in the first log line. Past five seconds, on we go
  // — /api/healthz turns healthy whenever the api comes up.
  if (await apiUp(5000)) console.log(`[talaria-ui] rust api ready on :${API_PORT}`)
  else console.warn(`[talaria-ui] rust api not answering on :${API_PORT} after 5s — /api/healthz will say when it is`)
}

await ensureRustApi()

// The probe is awaited BEFORE listen(), so anything it can wait on forever is
// something that can stop this process ever answering a request — including
// /api/healthz itself, which is what an orchestrator would use to notice. The
// handler bounds its own Postgres and Redis pings, but the probe is also what
// LOADS the route graph, and a module that stalls on import (or a driver that
// accepts a socket and then says nothing) is not covered by that.
//
// So: a deadline, and past it we carry on. Boot is not allowed to depend on a
// database being up. Everything downstream already degrades honestly — the
// proxy 502s, healthz answers 503 — but only if we get as far as listening.
//
// (The probe's old second job — warming the job modules so the scheduler
// handshake could find them — left with the cutover; what it loads now is the
// residents' graph, which is still every reason to run it before listen().)
const BOOT_PROBE_TIMEOUT_MS = 20_000

async function bootProbe() {
  const started = Date.now()
  try {
    let bell
    const deadline = new Promise((resolve) => {
      bell = setTimeout(() => resolve(null), BOOT_PROBE_TIMEOUT_MS)
    })
    const probe = await Promise.race([
      server
        .fetch(new Request('http://127.0.0.1/api/healthz', { method: 'GET' }))
        .then(async (r) => `${r.status} ${(await r.text()).slice(0, 200)}`),
      deadline,
    ]).finally(() => clearTimeout(bell))
    if (probe === null) {
      console.error(
        `[talaria-ui] boot probe /api/healthz did not answer within ${BOOT_PROBE_TIMEOUT_MS}ms —` +
          ' continuing to listen() anyway. Postgres or Redis is probably unreachable; /api/healthz will say which.',
      )
    } else {
      console.log(`[talaria-ui] boot probe /api/healthz → ${probe} (${Date.now() - started}ms)`)
    }
  } catch (err) {
    // A failed probe is not fatal: the graph may still have loaded, and each
    // dependency reports its own failures.
    console.error('[talaria-ui] boot probe failed (continuing anyway):', err)
  }
}

await bootProbe()

const httpServer = createServer(requestHandler)

// A redeploy sends SIGTERM. Tell the api child first — its own graceful path
// stops arming job runs and drains in-flight work (a job that ARCHIVES
// conversations or MESSAGES people must not be killed half a second in) —
// then stop accepting connections and let in-flight requests finish.
for (const signal of ['SIGTERM', 'SIGINT']) {
  process.on(signal, () => {
    if (shuttingDown) return
    shuttingDown = true
    console.log(`[talaria-ui] ${signal} — shutting down`)
    apiChild?.kill(signal)
    // An open keep-alive or SSE connection can hold close() indefinitely, and a
    // supervisor that has already sent SIGTERM will SIGKILL us shortly anyway.
    // Exit on our own terms instead, so the log says what happened.
    const forced = setTimeout(() => {
      console.warn('[talaria-ui] shutdown grace expired with connections still open — exiting')
      apiChild?.kill('SIGKILL')
      process.exit(0)
    }, 15_000)
    forced.unref()
    const closed = new Promise((resolve) => {
      httpServer.close((err) => {
        if (err) console.error('[talaria-ui] http close error:', err)
        resolve()
      })
    })
    // The api got its SIGTERM at the top; once the HTTP side is down, give it
    // a bounded window to finish its own drain before we exit — a bare `bun
    // run start` (no supervisor) would otherwise orphan it mid-drain.
    const apiDown = new Promise((resolve) => {
      if (!apiChild) return resolve()
      apiChild.once('close', resolve)
      const hard = setTimeout(() => {
        apiChild.kill('SIGKILL')
        resolve()
      }, 5_000)
      hard.unref()
    })
    Promise.all([apiDown, closed])
      .catch((err) => console.error('[talaria-ui] shutdown error:', err))
      .finally(() => process.exit(0))
  })
}
// The belt for every other exit path (an uncaught exception, a process.exit
// somewhere in the graph): never leave an api binary running behind us.
process.on('exit', () => apiChild?.kill('SIGKILL'))

httpServer.listen(port, host, () => {
  console.log(`[talaria-ui] listening on http://${host}:${port}`)
})
