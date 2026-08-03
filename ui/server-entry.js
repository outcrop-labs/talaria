// Production server: a Node HTTP server wrapping the TanStack Start fetch handler
// (dist/server/server.js) and serving the client assets (dist/client). Keeps the
// streaming pump so SSE chat responses flush incrementally.
import { createServer } from 'node:http'
import { readFile, stat } from 'node:fs/promises'
import { join, extname } from 'node:path'
import { fileURLToPath } from 'node:url'
import server from './dist/server/server.js'

const __dirname = fileURLToPath(new URL('.', import.meta.url))
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
    res.writeHead(response.status, Object.fromEntries(response.headers.entries()))
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

createServer(requestHandler).listen(port, host, () => {
  console.log(`[talaria-ui] listening on http://${host}:${port}`)
})
