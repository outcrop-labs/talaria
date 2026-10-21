#!/usr/bin/env node
// The MCP end-to-end suite's whole outside world, in one process on one port.
//
// WHAT IT SERVES (every path under https://<host>:<E2E_STUB_PORT>)
//   /v0.1/servers           a fake official registry: cursor paging (two
//                           entries a page, whatever `limit` asks — so the
//                           mirror's cursor walk is exercised, not assumed),
//                           `version=latest`, `updated_since`, `search`
//   /open/mcp               streamable-HTTP MCP, no auth: initialize,
//                           tools/list (3 tools), tools/call
//   /key/mcp                the same, 401 unless `X-Api-Key: stub-secret-4f9c2a`; it
//                           answers in SSE framing so the event-stream reader
//                           is on the path too
//   /oauth/mcp              the same behind OAuth 2.1: a 401 naming its
//                           protected-resource metadata, RFC 8414 metadata,
//                           dynamic client registration, an /authorize that
//                           302s straight back (no login page), a /token that
//                           verifies PKCE and issues the bearer this endpoint
//                           then accepts
//   /_e2e/calls             every tools/call that reached an upstream, so a
//                           test can prove a denied call never left Talaria
//   /_e2e/health            readiness
//
// WHY HTTPS. The mirror keeps only `https://` remotes (a plain-http endpoint is
//   never installable from the catalog), so a stub on http would be filtered
//   out before the suite could see it. Rather than teach the product a
//   test-only exception, the stub speaks TLS with a throwaway CA it mints at
//   start (openssl), and writes a CA bundle — the system roots plus that CA —
//   for the api to trust through SSL_CERT_FILE. The api still dials the stub
//   through its SSRF guard, so loopback needs TALARIA_FETCH_ALLOW_HOSTS too:
//   both are real operator settings, nothing here is a back door.
//
// RUN
//   node scripts/e2e/mcp-stubs.mjs          (bun works too)
//   env: E2E_STUB_PORT (5390) · E2E_STUB_HOST (127.0.0.1) · E2E_STUB_DIR (where
//   the CA, the bundle and stub.json land; default <OS temp>/talaria-e2e-stub-
//   <port>, the same dir on every start so `--init` and the server agree on
//   the CA) · E2E_STUB_TLS=0 serves plain http (registry-only debugging).
//   It prints one JSON line — {ready, base, bundle, …} — and writes the same to
//   <E2E_STUB_DIR>/stub.json. Stdlib only, like everything under scripts/.
//
//   `--init` mints the CA and the bundle, prints the env lines the api must
//   start with, and exits — the step before (re)starting a stack the suite
//   will run against (docs/E2E.md).

import { execFileSync } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import http from 'node:http'
import https from 'node:https'
import net from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const HOST = process.env.E2E_STUB_HOST || '127.0.0.1'
const PORT = Number(process.env.E2E_STUB_PORT || 5390)
const TLS = process.env.E2E_STUB_TLS !== '0'
// A STABLE default, keyed by the port: `--init` and the later server start
// are two processes, and the api trusts the CA the first one minted — a
// fresh directory per process would serve a leaf from a CA nobody trusts.
const DIR = process.env.E2E_STUB_DIR || join(tmpdir(), `talaria-e2e-stub-${PORT}`)
mkdirSync(DIR, { recursive: true })
const BASE = `${TLS ? 'https' : 'http'}://${HOST}:${PORT}`

/** The api key the /key endpoint accepts — the credentials test types it.
 *  Shaped so it cannot collide with any name the run registers (the server
 *  itself is `e2e-key`): the test asserts this exact string never comes back
 *  from the api, which only means something if nothing else contains it. */
const KEY = 'stub-secret-4f9c2a'
/** Registry entries change only when this process starts, which is what an
 *  incremental `updated_since` pass should see: everything once, then nothing. */
const STARTED = new Date().toISOString()
/** The page size the registry answers with, whatever `limit` asks — small on
 *  purpose so a five-entry catalog still takes three cursor hops. */
const PAGE = 2

// ── TLS material ─────────────────────────────────────────────────────────────

function openssl(args) {
  execFileSync('openssl', args, { stdio: ['ignore', 'ignore', 'pipe'] })
}

/** A throwaway CA and a leaf for the stub's host. Two certificates, not one
 *  self-signed leaf: rustls refuses a CA certificate presented as the server's
 *  own (CaUsedAsEndEntity), and `req -x509` marks its output a CA. */
function mintTls() {
  const ca = join(DIR, 'ca.pem')
  const caKey = join(DIR, 'ca.key')
  const leaf = join(DIR, 'leaf.pem')
  const leafKey = join(DIR, 'leaf.key')
  const csr = join(DIR, 'leaf.csr')
  const ext = join(DIR, 'leaf.ext')
  const ec = ['-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:prime256v1', '-nodes']
  // The CA is REUSED when the directory already holds one: the api reads
  // SSL_CERT_FILE when it starts, so a stack started against this bundle
  // must keep trusting the stub across stub restarts. The leaf is re-minted
  // every start — it is the CA the api trusts, not the leaf.
  if (!existsSync(ca) || !existsSync(caKey)) {
    openssl(['req', '-x509', ...ec, '-days', '365', '-subj', '/CN=Talaria e2e stub CA', '-keyout', caKey, '-out', ca])
  }
  openssl(['req', ...ec, '-subj', `/CN=${HOST}`, '-keyout', leafKey, '-out', csr])
  const san = net.isIP(HOST) ? `IP:${HOST}` : `DNS:${HOST}`
  writeFileSync(
    ext,
    [
      'basicConstraints=critical,CA:FALSE',
      'keyUsage=critical,digitalSignature',
      'extendedKeyUsage=serverAuth',
      `subjectAltName=${san},DNS:localhost,IP:127.0.0.1`,
    ].join('\n') + '\n',
  )
  openssl(['x509', '-req', '-in', csr, '-CA', ca, '-CAkey', caKey, '-CAcreateserial', '-days', '2', '-extfile', ext, '-out', leaf])
  // SSL_CERT_FILE REPLACES the system store for whoever reads it, so the
  // bundle carries the system roots too: an api trusting only the stub could
  // not reach anything real (the featured shelf's well-known lookups).
  const system = [
    '/etc/ssl/certs/ca-certificates.crt',
    '/etc/pki/tls/certs/ca-bundle.crt',
    '/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem',
    '/etc/ssl/cert.pem',
  ].find((p) => existsSync(p))
  const bundle = join(DIR, 'ca-bundle.pem')
  writeFileSync(bundle, (system ? readFileSync(system, 'utf8') + '\n' : '') + readFileSync(ca, 'utf8'))
  return { key: readFileSync(leafKey), cert: readFileSync(leaf), ca, bundle }
}

// ── A port nothing listens on, for the dead entry ────────────────────────────

/** Bound and released: nothing reopens it in the seconds a run lasts. */
function deadPort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer()
    s.once('error', reject)
    s.listen(0, HOST, () => {
      const { port } = s.address()
      s.close(() => resolve(port))
    })
  })
}

// ── The registry ─────────────────────────────────────────────────────────────

function registryEntries(dead) {
  const official = (version, isLatest) => ({
    'io.modelcontextprotocol.registry/official': {
      status: 'active',
      publishedAt: STARTED,
      updatedAt: STARTED,
      isLatest,
      version,
    },
  })
  const hosted = (slug, title, description, url) => ({
    server: {
      name: `e2e/${slug}`,
      title,
      description,
      version: '1.0.0',
      remotes: [{ type: 'streamable-http', url }],
    },
    _meta: official('1.0.0', true),
  })
  return [
    hosted('open', 'E2E Open', 'Talaria e2e stub: an MCP server that needs no credentials.', `${BASE}/open/mcp`),
    hosted('key', 'E2E Key', 'Talaria e2e stub: answers 401 until it is sent X-Api-Key.', `${BASE}/key/mcp`),
    hosted('oauth', 'E2E OAuth', 'Talaria e2e stub: OAuth 2.1 with dynamic client registration and PKCE.', `${BASE}/oauth/mcp`),
    hosted('dead', 'E2E Dead', 'Talaria e2e stub: an endpoint nothing listens on.', `https://${HOST}:${dead}/mcp`),
    {
      server: {
        name: 'e2e/package',
        title: 'E2E Package',
        description: 'Talaria e2e stub: a package-only entry (npm), never installed by the suite.',
        version: '1.0.0',
        packages: [{ registryType: 'npm', identifier: '@talaria-e2e/stub-server', version: '1.0.0', transport: { type: 'stdio' } }],
      },
      _meta: official('1.0.0', true),
    },
    // An OLDER version of the open server: only a caller that forgot
    // `version=latest` sees it, and the mirror would drop it (isLatest false).
    {
      server: {
        name: 'e2e/open',
        title: 'E2E Open (old)',
        description: 'Superseded version — present only without version=latest.',
        version: '0.9.0',
        remotes: [{ type: 'streamable-http', url: `${BASE}/open/mcp` }],
      },
      _meta: official('0.9.0', false),
    },
  ]
}

const cursorOf = (offset) => Buffer.from(`o:${offset}`).toString('base64url')
/** An unreadable cursor restarts the walk rather than failing it: a stack
 *  repointed at the stub may still hold the real registry's cursor. */
function offsetOf(cursor) {
  if (!cursor) return 0
  const m = /^o:(\d+)$/.exec(Buffer.from(cursor, 'base64url').toString())
  return m ? Number(m[1]) : 0
}

function registryList(url, entries) {
  const q = url.searchParams
  let rows = entries
  if (q.get('version') === 'latest') {
    rows = rows.filter((e) => e._meta['io.modelcontextprotocol.registry/official'].isLatest)
  }
  const since = q.get('updated_since')
  if (since) {
    const t = Date.parse(since)
    if (Number.isNaN(t)) return { status: 400, body: { error: `updated_since is not RFC 3339: ${since}` } }
    rows = rows.filter((e) => Date.parse(e._meta['io.modelcontextprotocol.registry/official'].updatedAt) > t)
  }
  const term = (q.get('search') || '').trim().toLowerCase()
  if (term) {
    rows = rows.filter((e) =>
      [e.server.name, e.server.title, e.server.description].some((f) => (f || '').toLowerCase().includes(term)),
    )
  }
  const limit = Math.max(1, Math.min(PAGE, Number(q.get('limit')) || PAGE))
  const start = offsetOf(q.get('cursor'))
  const page = rows.slice(start, start + limit)
  const next = start + limit < rows.length ? cursorOf(start + limit) : null
  return { status: 200, body: { servers: page, metadata: { count: page.length, ...(next ? { nextCursor: next } : {}) } } }
}

// ── The MCP servers ──────────────────────────────────────────────────────────

const TOOLS = [
  {
    name: 'echo',
    description: 'Answers with the text it was sent.',
    inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
  },
  {
    name: 'shout',
    description: 'Answers with the text it was sent, in capitals.',
    inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
  },
  {
    name: 'count',
    description: 'Answers with the length of the text it was sent.',
    inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
  },
]

/** Every tools/call that reached a stub, newest last. */
const calls = []

function rpcResult(id, result) {
  return { jsonrpc: '2.0', id, result }
}
function rpcError(id, code, message) {
  return { jsonrpc: '2.0', id: id ?? null, error: { code, message } }
}

/** One JSON-RPC message → the reply object, or null for a notification. */
function mcpAnswer(server, msg) {
  const id = msg.id
  switch (msg.method) {
    case 'initialize':
      return rpcResult(id, {
        protocolVersion: msg.params?.protocolVersion || '2025-06-18',
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: `e2e-${server}`, version: '1.0.0' },
      })
    case 'ping':
      return rpcResult(id, {})
    case 'tools/list':
      return rpcResult(id, { tools: TOOLS })
    case 'tools/call': {
      const name = msg.params?.name
      const text = String(msg.params?.arguments?.text ?? '')
      calls.push({ server, tool: name, text, at: new Date().toISOString() })
      if (name === 'echo') return rpcResult(id, { content: [{ type: 'text', text }] })
      if (name === 'shout') return rpcResult(id, { content: [{ type: 'text', text: text.toUpperCase() }] })
      if (name === 'count') return rpcResult(id, { content: [{ type: 'text', text: String(text.length) }] })
      return rpcError(id, -32602, `unknown tool: ${name}`)
    }
    default:
      if (id === undefined || String(msg.method || '').startsWith('notifications/')) return null
      return rpcError(id, -32601, `method not found: ${msg.method}`)
  }
}

function serveMcp(server, req, res, body, { sse = false } = {}) {
  if (req.method !== 'POST') return send(res, 405, { error: 'POST only — streamable HTTP without a GET stream' })
  let msg
  try {
    msg = JSON.parse(body)
  } catch {
    return send(res, 400, rpcError(null, -32700, 'parse error'))
  }
  const reply = mcpAnswer(server, msg)
  const session = req.headers['mcp-session-id'] || `e2e-${server}-${randomBytes(6).toString('hex')}`
  if (reply === null) {
    res.writeHead(202, { 'mcp-session-id': session })
    return res.end()
  }
  if (sse) {
    res.writeHead(200, { 'content-type': 'text/event-stream', 'mcp-session-id': session })
    return res.end(`event: message\ndata: ${JSON.stringify(reply)}\n\n`)
  }
  return send(res, 200, reply, { 'mcp-session-id': session })
}

// ── The OAuth half ───────────────────────────────────────────────────────────

const clients = new Map() // client_id → { redirect_uris }
const codes = new Map() // code → { clientId, redirectUri, challenge, resource }
const tokens = new Set() // live access tokens
const refreshTokens = new Set()

const PR_PATH = '/.well-known/oauth-protected-resource/oauth/mcp'
const issuer = () => `${BASE}/oauth`
function asMetadata() {
  return {
    issuer: issuer(),
    authorization_endpoint: `${BASE}/oauth/authorize`,
    token_endpoint: `${BASE}/oauth/token`,
    registration_endpoint: `${BASE}/oauth/register`,
    response_types_supported: ['code'],
    grant_types_supported: ['authorization_code', 'refresh_token'],
    code_challenge_methods_supported: ['S256'],
    token_endpoint_auth_methods_supported: ['none', 'client_secret_post'],
    scopes_supported: ['e2e.read'],
    service_documentation: `${BASE}/oauth/docs`,
  }
}

const s256 = (verifier) => createHash('sha256').update(verifier).digest('base64url')

function oauthToken(res, form) {
  const fail = (error, description) => send(res, 400, { error, error_description: description })
  const grant = form.get('grant_type')
  if (grant === 'refresh_token') {
    const rt = form.get('refresh_token') || ''
    if (!refreshTokens.has(rt)) return fail('invalid_grant', 'unknown refresh token')
    const access = `e2e-at-${randomBytes(16).toString('hex')}`
    tokens.add(access)
    return send(res, 200, { access_token: access, token_type: 'Bearer', expires_in: 3600, refresh_token: rt, scope: 'e2e.read' })
  }
  if (grant !== 'authorization_code') return fail('unsupported_grant_type', `grant_type ${grant} is not supported`)
  const code = form.get('code') || ''
  const issued = codes.get(code)
  if (!issued) return fail('invalid_grant', 'unknown or already-used authorization code')
  codes.delete(code) // single use, like the real thing
  if (issued.clientId !== form.get('client_id')) return fail('invalid_grant', 'the code was issued to another client')
  if (issued.redirectUri !== form.get('redirect_uri')) return fail('invalid_grant', 'redirect_uri does not match the authorization request')
  const verifier = form.get('code_verifier') || ''
  if (!verifier || s256(verifier) !== issued.challenge) return fail('invalid_grant', 'PKCE verification failed')
  const access = `e2e-at-${randomBytes(16).toString('hex')}`
  const refresh = `e2e-rt-${randomBytes(16).toString('hex')}`
  tokens.add(access)
  refreshTokens.add(refresh)
  return send(res, 200, { access_token: access, token_type: 'Bearer', expires_in: 3600, refresh_token: refresh, scope: 'e2e.read' })
}

function oauthAuthorize(res, url) {
  const q = url.searchParams
  const page = (status, text) => {
    res.writeHead(status, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(text)
  }
  const client = clients.get(q.get('client_id') || '')
  if (!client) return page(400, 'unknown client_id — register first')
  const redirectUri = q.get('redirect_uri') || ''
  if (!client.redirect_uris.includes(redirectUri)) return page(400, `redirect_uri ${redirectUri} is not registered for this client`)
  if (q.get('response_type') !== 'code') return page(400, 'response_type must be code')
  if (q.get('code_challenge_method') !== 'S256' || !q.get('code_challenge')) return page(400, 'PKCE S256 is required')
  const code = `e2e-code-${randomBytes(12).toString('hex')}`
  codes.set(code, { clientId: q.get('client_id'), redirectUri, challenge: q.get('code_challenge'), resource: q.get('resource') })
  const back = new URL(redirectUri)
  back.searchParams.set('code', code)
  if (q.get('state')) back.searchParams.set('state', q.get('state'))
  res.writeHead(302, { location: back.toString() })
  res.end()
}

// ── HTTP plumbing ────────────────────────────────────────────────────────────

function send(res, status, body, headers = {}) {
  res.writeHead(status, { 'content-type': 'application/json', ...headers })
  res.end(JSON.stringify(body))
}

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = []
    req.on('data', (c) => chunks.push(c))
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')))
  })
}

/** What the api under test must be started with to reach this stub. */
function apiEnv(bundle) {
  return [
    `TALARIA_FETCH_ALLOW_HOSTS=${HOST}`,
    ...(bundle ? [`SSL_CERT_FILE=${bundle}`] : []),
    `TALARIA_MCP_REGISTRY_URL=${BASE}/v0.1/servers`,
  ]
}

async function main() {
  if (process.argv.includes('--init')) {
    const tls = TLS ? mintTls() : null
    console.log(apiEnv(tls?.bundle).join('\n'))
    return
  }
  const dead = await deadPort()
  const entries = registryEntries(dead)
  const tls = TLS ? mintTls() : null

  const handler = async (req, res) => {
    const url = new URL(req.url || '/', BASE)
    const body = await readBody(req)
    const path = url.pathname
    // One line per request (never a header value or a body): the log CI
    // uploads on a red run is how a failed discovery or OAuth hop is read.
    res.on('finish', () => {
      if (path !== '/_e2e/health') console.log(`[e2e-stubs] ${req.method} ${path} → ${res.statusCode}`)
    })
    try {
      if (path === '/_e2e/health') return send(res, 200, { ok: true, base: BASE })
      if (path === '/_e2e/calls') return send(res, 200, { calls })

      if (path === '/v0.1/servers' && req.method === 'GET') {
        const out = registryList(url, entries)
        return send(res, out.status, out.body)
      }

      if (path === '/open/mcp') return serveMcp('open', req, res, body)

      if (path === '/key/mcp') {
        if (req.headers['x-api-key'] !== KEY) {
          return send(res, 401, rpcError(null, -32001, 'missing or wrong X-Api-Key'), {
            'www-authenticate': 'ApiKey realm="e2e", header="X-Api-Key"',
          })
        }
        return serveMcp('key', req, res, body, { sse: true })
      }

      if (path === '/oauth/mcp') {
        const auth = String(req.headers.authorization || '')
        const bearer = auth.startsWith('Bearer ') ? auth.slice(7).trim() : ''
        if (!tokens.has(bearer)) {
          return send(res, 401, rpcError(null, -32001, 'authorization required'), {
            'www-authenticate': `Bearer resource_metadata="${BASE}${PR_PATH}", scope="e2e.read"`,
          })
        }
        return serveMcp('oauth', req, res, body)
      }
      // RFC 9728, path-aware: the metadata names THIS resource. The bare
      // origin-level document is deliberately absent (404 below) — the key
      // server shares this origin, and a client that falls back to the origin
      // must not mistake it for an OAuth server.
      if (path === PR_PATH) {
        return send(res, 200, {
          resource: `${BASE}/oauth/mcp`,
          authorization_servers: [issuer()],
          scopes_supported: ['e2e.read'],
          bearer_methods_supported: ['header'],
        })
      }
      // RFC 8414 for an issuer with a path: the well-known segment sits
      // between host and path; the "appended" shapes answer too. The OIDC
      // appended one (<issuer>/.well-known/openid-configuration) is the only
      // candidate Talaria builds from the issuer URL verbatim — the others
      // are rebuilt from scheme + host and lose a non-default port
      // (talaria-mcp-oauth `as_metadata_candidates`), so against a stub on
      // :5390 it is the one discovery actually reaches.
      if (
        path === '/.well-known/oauth-authorization-server/oauth' ||
        path === '/oauth/.well-known/oauth-authorization-server' ||
        path === '/oauth/.well-known/openid-configuration'
      ) {
        return send(res, 200, asMetadata())
      }
      if (path === '/oauth/register' && req.method === 'POST') {
        let reg
        try {
          reg = JSON.parse(body || '{}')
        } catch {
          return send(res, 400, { error: 'invalid_client_metadata', error_description: 'body is not JSON' })
        }
        const uris = Array.isArray(reg.redirect_uris) ? reg.redirect_uris.filter((u) => typeof u === 'string') : []
        if (!uris.length) return send(res, 400, { error: 'invalid_redirect_uri', error_description: 'redirect_uris is required' })
        const clientId = `e2e-client-${randomBytes(8).toString('hex')}`
        clients.set(clientId, { redirect_uris: uris })
        return send(res, 201, {
          client_id: clientId,
          client_id_issued_at: Math.floor(Date.now() / 1000),
          client_name: reg.client_name || null,
          redirect_uris: uris,
          grant_types: ['authorization_code', 'refresh_token'],
          response_types: ['code'],
          token_endpoint_auth_method: 'none',
        })
      }
      if (path === '/oauth/authorize' && req.method === 'GET') return oauthAuthorize(res, url)
      if (path === '/oauth/token' && req.method === 'POST') return oauthToken(res, new URLSearchParams(body))
      if (path === '/oauth/docs') {
        res.writeHead(200, { 'content-type': 'text/plain' })
        return res.end('The Talaria e2e OAuth stub. Nothing to read.')
      }
      return send(res, 404, { error: `no stub at ${req.method} ${path}` })
    } catch (e) {
      return send(res, 500, { error: String(e?.message || e) })
    }
  }

  const server = tls ? https.createServer({ key: tls.key, cert: tls.cert }, handler) : http.createServer(handler)
  server.on('error', (e) => {
    console.error(`[e2e-stubs] cannot listen on ${HOST}:${PORT}: ${e.message}`)
    process.exit(1)
  })
  server.listen(PORT, HOST, () => {
    const info = {
      ready: true,
      base: BASE,
      registry: `${BASE}/v0.1/servers`,
      tls: !!tls,
      ca: tls?.ca ?? null,
      bundle: tls?.bundle ?? null,
      deadPort: dead,
      dir: DIR,
      apiEnv: apiEnv(tls?.bundle),
    }
    writeFileSync(join(DIR, 'stub.json'), JSON.stringify(info, null, 2) + '\n')
    console.log(JSON.stringify(info))
  })
  for (const sig of ['SIGINT', 'SIGTERM']) process.on(sig, () => server.close(() => process.exit(0)))
}

await main()
