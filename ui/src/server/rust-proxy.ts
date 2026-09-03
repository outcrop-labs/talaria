// The API boundary, post-cutover (docs/RUST-MIGRATION.md): every /api/*
// request is the Rust api's, same-origin, on TALARIA_RUST_API_URL. The batch
// lists that accumulated here route by route are gone — the TS route files
// they shadowed are deleted with them. What remains in routes/api/ is exactly
// the residents STAY_TS holds back below.
//
// The Rust api is ASSUMED, not opted into: unset env forwards to the default
// loopback address, so a fresh checkout proxies the moment an api is listening
// — no wiring step exists to forget (the day-long API-DARK dev box this
// default deleted was a real one). TALARIA_RUST_API_URL moves the target;
// the literal `off` stands the hop down for the one posture that wants no
// api behind this process at all (tests, a deliberately-unproxied install).
// No silent fallback either way: if the env names a Rust api and it is down,
// the answer is a 502, not a quiet re-serve from TS. A fallback would make
// "both runtimes serve one group" the failure mode instead of the invariant.
//
// The target is operator env (same standing as the gateway's configured
// endpoints, not safeFetch's user-supplied URLs); the fetch below is a
// server-to-server hop inside `ui/src/server/**`, the SDK-door invariant's
// explicit non-browser territory.

import { json } from './http'

// The one prefix. '/api' itself (no slash) stays a TS 404, exactly as before
// the cutover — the SPA shell and the residents are this process's, and no
// route ever lived at the bare path.
const PREFIXES = ['/api/'] as const

// The residents: paths that stay on TS even though the prefix covers them.
// Every one is PERMANENT, not backlog:
//   rule 10 — app modules are app authors' TS/node code, dispatched
//   in-process. The app-server gateway (/api/apps/{slug}/…, apps.$app.$.ts)
//   and the app-MCP dispatch (/api/mcp/gw/app-…, the app branch of
//   mcp.gw.$server.ts) must run in the runtime that can actually load the
//   modules; Rust answers a fixed sentence if hit directly (dev setups that
//   point agents straight at :5274), but the proxy never routes an app
//   server's traffic anywhere else. Bare /api/apps is NOT a resident — it is
//   the discovery listing, and it serves from Rust like everything else.
//   admin.update — it rebuilds ui/dist and restarts the bun process it runs
//   IN: it is the deployer of the TS half itself, so the other runtime
//   cannot serve it (the Rust binary's own update path is that route's
//   redesign, not a port).
//   healthz — the app process's own liveness answer; the Rust api carries
//   its own at the same path for whoever asks it there.
const STAY_TS = [
  /^\/api\/mcp\/gw\/app-/,
  /^\/api\/admin\/update$/,
  /^\/api\/apps\//,
  /^\/api\/healthz$/,
] as const

// The default target: in-box loopback on the same port `talaria dev` and
// server-entry.js bind their api to. Both wire the env explicitly when they
// own the api; this default covers everyone else — a bare `vite dev`, a
// hand-started install — so "first spin-up assumes Rust" is a property of the
// process, not of its operator's memory.
export const DEFAULT_RUST_API_URL = 'http://127.0.0.1:5274'

// Read per call, not at module load: the unset→set flip (dev wiring, tests)
// must not depend on which module graph got the frozen copy. Returns the
// effective URL, or undefined ONLY for `off` — the hop stood down on purpose.
export const rustApiUrl = (): string | undefined => {
  const v = process.env.TALARIA_RUST_API_URL?.trim()
  if (v === 'off') return undefined
  return v || DEFAULT_RUST_API_URL
}

// Hop-by-hop headers die at this hop: the Rust api is a fresh origin with its
// own framing. Everything not listed here rides through untouched — the
// Authorization header above all, plus any content-type the caller sent.
const HOP_BY_HOP = new Set(['host', 'connection', 'content-length', 'transfer-encoding', 'keep-alive', 'upgrade', 'proxy-authorization', 'proxy-authenticate', 'te', 'trailer'])

// The response allow-list is the inverse posture: nothing the Rust api says
// about itself (its server header, its date) is ours to re-claim under this
// origin. These are the ones callers can act on — `location` because the
// OAuth routes answer in redirects, and the conversation/message ids because
// /api/chat answers a NEW conversation's id in a header: the SPA learns it
// from X-Conversation-Id, and stripping it would strand a first turn.
// `pragma` and `referrer-policy` joined with the secrets family: reveal and
// git-credential answer with no-store/no-cache/no-referrer triads, and an
// allow-list that eats two of the three is a cache hint waiting to happen.
// `content-disposition` rides with the uploads: serve_upload puts the file's
// name there, and stripping it turned every download into a nameless blob —
// the "attachments are broken" report's download half. Its sandbox belt
// (`content-security-policy` + `x-content-type-options`) crosses for the
// same reason: the api buckles it so a served file can't execute where it
// lands, and the belt is worthless if it stops at this boundary.
const RESPONSE_HEADERS = ['content-type', 'cache-control', 'retry-after', 'x-request-id', 'location', 'x-conversation-id', 'x-message-id', 'pragma', 'referrer-policy', 'content-disposition', 'content-security-policy', 'x-content-type-options']

export async function maybeProxy(request: Request, pathname: string): Promise<Response | null> {
  const base = rustApiUrl()
  if (base === undefined || STAY_TS.some((r) => r.test(pathname))) return null
  if (!PREFIXES.some((p) => pathname.startsWith(p))) return null

  const incoming = new URL(request.url)
  const target = base + incoming.pathname + incoming.search

  const headers = new Headers()
  request.headers.forEach((value, name) => {
    if (!HOP_BY_HOP.has(name.toLowerCase())) headers.set(name, value)
  })
  // The Rust api sees the request without this origin, and the OAuth redirect
  // URIs are derived from it. Forward whatever the caller's own proxy stated,
  // else synthesize it from the incoming URL — exactly the chain TS's
  // resolveOrigin prefers, so both runtimes compute the same redirect_uri.
  if (!headers.has('x-forwarded-proto')) headers.set('x-forwarded-proto', incoming.protocol.replace(':', ''))
  if (!headers.has('x-forwarded-host')) headers.set('x-forwarded-host', incoming.host)

  const res = await fetch(target, {
    method: request.method,
    headers,
    body: request.method === 'GET' || request.method === 'HEAD' ? undefined : await request.arrayBuffer(),
    signal: request.signal,
    // The Rust api is loopback by design; a redirect would mean it is not
    // the api we configured — follow nothing.
    redirect: 'manual',
  }).catch((e: Error) => e)

  if (res instanceof Error) {
    // Fixed sentence, the one this boundary already uses — the fetch error
    // names the Rust api's host:port, which is topology, not a key holder's
    // business. Same shape as llm.v1.chat.completions.ts's own 502.
    console.error(`[rust-proxy] ${pathname} unreachable: ${res.message}`)
    return json({ error: { message: 'upstream unreachable' } }, { status: 502 })
  }

  const out = new Headers()
  for (const name of RESPONSE_HEADERS) {
    const v = res.headers.get(name)
    if (v !== null) out.set(name, v)
  }
  // Set-Cookie rides through plural-safe: get() would comma-join multiple
  // cookies into one corrupt value (Set-Cookie may itself contain commas), and
  // an auth plane that can't clear its cookie through the boundary is not
  // ported. The login/OAuth routes land here too, and they set two.
  for (const c of res.headers.getSetCookie()) out.append('set-cookie', c)
  // The body streams through with backpressure — both callers (vite dev,
  // server-entry.js) pump Response.body; an SSE relay must not buffer.
  return new Response(res.body, { status: res.status, headers: out })
}
