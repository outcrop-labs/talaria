// The coexistence switch for the TS→Rust migration (docs/RUST-MIGRATION.md).
// Route groups migrate one prefix at a time; while a prefix lives here the TS
// server forwards it, same-origin, to the Rust api on TALARIA_RUST_API_URL.
// Unset env = this module forwards nothing and behavior is byte-identical —
// prod environments that never set it never think about Rust.
//
// No silent fallback, on purpose: if the env names a Rust api and it is down,
// the answer is a 502, not a quiet re-serve from TS. A fallback would make
// "both runtimes serve one group" the failure mode instead of the invariant.
//
// The target is operator env (same standing as the gateway's configured
// endpoints, not safeFetch's user-supplied URLs); the fetch below is a
// server-to-server hop inside `ui/src/server/**`, the SDK-door invariant's
// explicit non-browser territory.

import { json } from './http'

// The compiled switch list. A prefix joins when its Rust port is verified
// byte-compatible, and leaves when the TS route files it shadows are deleted.
const PREFIXES = [
  '/api/llm/v1/',
  '/api/auth/session',
  '/api/auth/logout',
  '/api/auth/password',
  '/api/auth/providers',
  '/api/auth/claim',
  // Prefix, not exact: '/api/auth/google' also matches its own /callback —
  // the only other route under this path (the CONNECT flow lives elsewhere).
  '/api/auth/google',
  '/api/users',
  '/api/activity',
  '/api/cost',
  // The whole keys group: keys.ts + keys.$id.ts are the only routes under
  // this path, so the prefix is the group. Same for teams — the list, the
  // {id} rename/delete, and the members sub-route are one plane. Same for
  // workflows: the list and the {id} patch/remove are the whole group. Same
  // for notifications — one route file, three methods.
  '/api/keys',
  '/api/teams',
  '/api/workflows',
  '/api/notifications',
  // The model-identity plane: the picker catalog (models.ts) and the effort
  // ladder (models.efforts.ts) are the only routes under /api/models, and
  // they crossed together — the picker's rows and the composer's effort feed
  // read the same persona/catalog state.
  '/api/models',
  // The boards group, whole: the list/create, the {id} board, and every
  // sub-route (members, labels, statuses, tasks, agents, templates, views,
  // events) — plus the tasks group a board hands off to: the ticket, its
  // comments, dependencies, review gate, usage, and watchers. Statuses and
  // tasks were the last two files under either path; both are all-Rust now.
  '/api/boards',
  '/api/tasks',
  // The admin console's wave-1 groups. Each prefix IS the whole group — no
  // TS sub-routes hide under any of these paths. '/api/admin/google-client'
  // covers its own /login sibling. Still TS: admin/invites (createInvite
  // sends email), admin/model-fitness (the probe suite's plane), and the
  // rest of admin/*.
  '/api/agent-role-templates',
  '/api/admin/password-accounts',
  '/api/admin/google-client',
  '/api/admin/instance',
  '/api/admin/permissions',
] as const

// Whole-path migrations: the ROUTE is the group, because everything under it
// besides the route itself still belongs to TS. '/api/agents' has register +
// heartbeat sub-routes (the fleet plane, a later batch), and '/api/apps' is
// the app-server gateway (apps.$app.$ dispatches into app server modules —
// TS until cutover by construction). '/api/me' is exact for the same reason:
// me.mcp and me.assistant are their own planes (fleet, agents) that migrate
// whole with those batches. '/api/me/events' is whole-path too — this
// person's own SSE firehose, which crossed with the realtime slice while the
// rest of me stayed. A startsWith entry here would strand those on a Rust 404.
const EXACT = new Set(['/api/agents', '/api/apps', '/api/me', '/api/me/events', '/api/admin/model-roles'])

// Parameterized whole-route migrations: the route crossed, but its siblings
// under the same path have not, so neither EXACT (the id is in the path) nor
// PREFIXES (it would strand the siblings on a Rust 404) can express it. Each
// entry is one TS route file's path shape, anchored both ends.
const SHAPES = [
  // runs.$id.events.ts — the run's live SSE view, gated by the run's read
  // ACL. The rest of /api/runs (the list, the detail, cancel, decide) is
  // still TS until the runs surface crosses as a group.
  /^\/api\/runs\/[^/]+\/events$/,
] as const

// Read per call, not at module load: the unset→set flip (dev wiring, tests)
// must not depend on which module graph got the frozen copy.
const rustApiUrl = (): string | undefined => process.env.TALARIA_RUST_API_URL || undefined

// Hop-by-hop headers die at this hop: the Rust api is a fresh origin with its
// own framing. Everything not listed here rides through untouched — the
// Authorization header above all, plus any content-type the caller sent.
const HOP_BY_HOP = new Set(['host', 'connection', 'content-length', 'transfer-encoding', 'keep-alive', 'upgrade', 'proxy-authorization', 'proxy-authenticate', 'te', 'trailer'])

// The response allow-list is the inverse posture: nothing the Rust api says
// about itself (its server header, its date) is ours to re-claim under this
// origin. These five are the ones callers can act on — `location` because the
// OAuth routes answer in redirects.
const RESPONSE_HEADERS = ['content-type', 'cache-control', 'retry-after', 'x-request-id', 'location']

export async function maybeProxy(request: Request, pathname: string): Promise<Response | null> {
  const base = rustApiUrl()
  if (!base || (!EXACT.has(pathname) && !PREFIXES.some((p) => pathname.startsWith(p)) && !SHAPES.some((r) => r.test(pathname)))) return null

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
