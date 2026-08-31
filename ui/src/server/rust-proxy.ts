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
  // The research group: research.ts (the list + start) is the only route at
  // the bare path. The parameterized half of the family ({id}, {id}/members,
  // {id}/conversation, {id}/decide) crossed with it and is matched by SHAPES
  // below — a prefix alone would 404 nothing, but the shapes keep the two
  // halves of one family in one file, read together.
  '/api/research',
  // The rag group: the collection registry (list/create, {id} bindings and
  // delete) and the search the search_knowledge MCP tool rides. The whole
  // family under this path — the admin console for it lives at
  // /api/admin/rag, already in EXACT.
  '/api/rag',
  // The knowledgebase plane, whole: spaces and their doc trees, docs with
  // comments/backlinks/move/live-presence, full-text search, and the two
  // public slug reads. Every kb.* route file crossed together — the ACL
  // engine (kb_perms) is shared shape with the rag collections registry, so
  // the family crossed as one.
  '/api/kb',
  // The artifacts plane — the Files surface. THREE prefixes, not one:
  // '/api/artifacts' does not prefix-match '/api/artifact-folders' (the
  // match is character-by-character: 's' vs '-' at position 13), and
  // '/api/uploads' is its own family the file artifacts point at. Links,
  // public slugs and the Google Drive export all live under the artifacts
  // prefix proper.
  '/api/artifacts',
  '/api/artifact-folders',
  '/api/uploads',
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
  // The comms plane, whole: the channel list/create, one channel and its
  // members/agents/read-cursor, messages (with edit/delete/reactions/threads),
  // the Relay conclude, the live SSE events, and the plan-draft mount that
  // crossed earlier (its SHAPES entry below is subsumed by this prefix).
  '/api/channels',
  // The conversations family: the list and the {id} detail/rename. The
  // durable chat itself (/api/chat) is whole-path — see EXACT — and /api/dms
  // is its own one-route family.
  '/api/conversations',
  '/api/dms',
  // The inbox.focus family, whole: the queue read, the badge summary, the
  // viewed/snooze state write, the assistant actions (with the confirmation
  // reissue), the SSE command stream, and the segmented conversation picker
  // with its {id} timeline/archive. Every inbox.focus* route file crossed
  // together — the focus engine's process-local lock spans the command stream
  // and the state route, so the family is one plane.
  '/api/inbox/focus',
  // The brief family, whole: the document read (with its three kinds of
  // nothing), the read cursor, the owner's verdict on a line, and the
  // delegation trio (grants list, grant/revoke, decide a parked reply). The
  // engine's sweep half crossed earlier with the scheduler — this adds the
  // reader plane, and the read's sweep-if-due must land on the same Rust
  // process as the engine that owns it.
  '/api/brief',
  // The secrets family, whole: the working secrets a person saves and reads
  // back, their folders, the one reveal verb, sharing, the one-shot relay,
  // and git's credential helper door. The vault engine crossed with them —
  // and the resolve path it serves is what MCP tool-call substitution uses,
  // so the whole feature reads and writes one Rust process's tables.
  '/api/secrets',
  // The integrations/google family, whole: both connect/callback pairs
  // (personal and org), the org targets/provisioning/health, the
  // pending-action approval queue, and the per-surface reads and mutations
  // (calendar, drive, gmail) in both flavors — as the user, and as the agent
  // acting for its owner or the org. The family crossed with the google
  // engines; every /api/integrations/* route file is in this set.
  '/api/integrations',
  // The workbench family, whole: the profile registry, the per-repo git flow,
  // the org GitHub connection, the harness registry, the human side of
  // workbench jobs (ticket strip + approve/reject/merge-to-testing), the
  // repo-creation approval queue, and the per-agent repo grants — all seven
  // route files. The workbench MCP dispatcher the fleet's agents speak
  // crossed with them in the same crate.
  '/api/workbench',
] as const

// Whole-path migrations: the ROUTE is the group, because everything under it
// besides the route itself still belongs to TS. '/api/agents' has register +
// heartbeat sub-routes (the fleet plane, a later batch), and '/api/apps' is
// the app-server gateway (apps.$app.$ dispatches into app server modules —
// the gateway is host plumbing and may cross some day, but the modules it
// dispatches into are app authors' TS/node code and stay TS by the port's
// rule 10, not by backlog). '/api/me' is exact for the same reason:
// me.mcp and me.assistant are their own planes (fleet, agents) that migrate
// whole with those batches. '/api/me/events' is whole-path too — this
// person's own SSE firehose, which crossed with the realtime slice while the
// rest of me stayed. The two fleet routes are exact because the fleet family
// is 20 TS route files and only the hire's create/list crossed — the other
// 18 (status, stop/start, logs, env) still serve from TS, and a prefix here
// would strand every one of them on a Rust 404.
const EXACT = new Set([
  '/api/agents',
  '/api/apps',
  '/api/me',
  '/api/me/events',
  '/api/admin/model-roles',
  // The retrieval console. EXACT because the /api/admin/rag path names one
  // route file; the bare /api/rag/* family (collections, search) is a
  // different path that crossed later and lives in PREFIXES.
  '/api/admin/rag',
  '/api/fleet/create',
  '/api/fleet/hires',
  // The version-history read plane — one route file over two stores
  // (internal_versions snapshots, agent_versions). No sub-routes hide under
  // this path, and nothing else in the tree starts with "/api/history".
  '/api/history',
  // The durable chat: one POST route, whole-path — nothing else lives at
  // /api/chat, and a prefix would be the same set anyway.
  '/api/chat',
])

// Parameterized whole-route migrations: the route crossed, but its siblings
// under the same path have not, so neither EXACT (the id is in the path) nor
// PREFIXES (it would strand the siblings on a Rust 404) can express it. Each
// entry is one TS route file's path shape, anchored both ends.
const SHAPES = [
  // runs.$id.events.ts — the run's live SSE view, gated by the run's read
  // ACL. The rest of /api/runs (the list, the detail, cancel, decide) is
  // still TS until the runs surface crosses as a group.
  /^\/api\/runs\/[^/]+\/events$/,
  // plans.$id.draft.ts — the plan-draft plane's other half. The channels
  // mount crossed with the channels family (now a PREFIXES entry above); the
  // plans family (messages, title, doc) stays TS until the chat batch, so a
  // PREFIXES entry would strand them on a Rust 404.
  /^\/api\/plans\/[^/]+\/draft$/,
  // research.ts + research.$id{,.members,.conversation,.decide}.ts — the
  // research plane crossed whole (domain, def and routes together). The
  // bare /api/research is a PREFIXES entry (see above); these shapes are the
  // parameterized rest of the family. Nothing else lives under /api/research,
  // so the shape set IS the family.
  /^\/api\/research\/[^/]+$/,
  /^\/api\/research\/[^/]+\/members$/,
  /^\/api\/research\/[^/]+\/conversation$/,
  /^\/api\/research\/[^/]+\/decide$/,
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
// origin. These are the ones callers can act on — `location` because the
// OAuth routes answer in redirects, and the conversation/message ids because
// /api/chat answers a NEW conversation's id in a header: the SPA learns it
// from X-Conversation-Id, and stripping it would strand a first turn.
// `pragma` and `referrer-policy` joined with the secrets family: reveal and
// git-credential answer with no-store/no-cache/no-referrer triads, and an
// allow-list that eats two of the three is a cache hint waiting to happen.
const RESPONSE_HEADERS = ['content-type', 'cache-control', 'retry-after', 'x-request-id', 'location', 'x-conversation-id', 'x-message-id', 'pragma', 'referrer-policy']

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
