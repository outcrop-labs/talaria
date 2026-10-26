// What the app gateway hands an app's server besides the gate's answer: the
// sub-path as segments, the public origin, a client for the platform API that
// runs as the caller, the workspace secrets the app was given, and the
// request's abort signal. Kept apart from the route file so each piece is
// pinned on its own (app-context.test.ts) and the route stays the wiring.
import { HttpError } from '@/lib/fetch-json'
import type { AppMcpCaller } from '@/sdk/server'
import { appAgents, type AppAgents } from './app-agents'
import { appHarnessRunner, type AppHarnessRunner, type HarnessActor } from './app-harness-runner'
import type { OrgToolCall } from './app-org-tools'
import { storeFor, type AppStore } from './app-store'
import { getAuthConfig } from './auth/config'
import { rustApiUrl } from './rust-proxy'
import { resolveAppSecret } from './workspace-secrets'

export { HttpError }

// ── Who started the app's code ───────────────────────────────────────────────

/** What the host authenticated before an app's code ran — the actor every
 *  context's capabilities are bound to. */
export type AppActor =
  /** A signed-in person, through `/api/apps/<slug>/*`. */
  | { kind: 'request'; user: { id: string; role: string } }
  /** An agent calling one of the app's MCP tools: who it is, whether the
   *  gateway proved it, and the person it acts for. */
  | { kind: 'mcp'; caller: AppMcpCaller; actingUserId: string | null }
  /** A schedule or a platform event (`talaria.json` `triggers`): the app
   *  itself, with no user. */
  | { kind: 'trigger'; trigger: string }

/** What every app context carries whatever started it. */
export interface AppServices {
  store: AppStore
  agents: AppAgents
  harness: AppHarnessRunner
}

/** What a context knows beyond the actor, for the bound harness runner. */
export interface AppServiceEdges {
  /** A request's: the agent ids the signed-in person may use — the api's
   *  `usable_agent_gate`, asked with their own session. Absent, a request's
   *  run may pin no agent. */
  usableAgents?: () => Promise<string[]>
  /** An MCP call's: how a loop's `org` tool is called, as the calling agent. */
  orgTools?: OrgToolCall | null
}

/** The actor as the bound harness runner reads it. */
function harnessActorOf(actor: AppActor, edges: AppServiceEdges): HarnessActor {
  switch (actor.kind) {
    case 'request':
      return {
        kind: 'request',
        user: { id: actor.user.id, role: actor.user.role === 'admin' ? 'admin' : 'member' },
        usableAgents: edges.usableAgents ?? (async () => []),
      }
    case 'mcp':
      return { kind: 'mcp', caller: actor.caller, actingUserId: actor.actingUserId }
    case 'trigger':
      return { kind: 'trigger' }
  }
}

/** THE ONE FACTORY the request, MCP-tool and trigger contexts are built
 *  through: everything that is bound to the actor, built from the actor, in
 *  one place. A capability added for every context is added here, once.
 *  `signal` is the context's own — the request's disconnect, the tool's
 *  deadline, the trigger's. */
export function appServices(app: string, actor: AppActor, signal?: AbortSignal, edges: AppServiceEdges = {}): AppServices {
  return {
    store: storeFor(app),
    agents: appAgents({ app, actor, signal }),
    harness: appHarnessRunner({
      app,
      actor: harnessActorOf(actor, edges),
      ...(edges.orgTools !== undefined ? { orgTools: edges.orgTools } : {}),
      ...(signal ? { signal } : {}),
    }),
  }
}

/** The app's sub-path as decoded segments, or why the path is refused.
 *
 *  `rawPathname` is the request's own `URL.pathname`, still percent-encoded;
 *  the first three segments are `api`, `apps` and the slug. Each segment is
 *  decoded exactly once, so `%25` is a literal `%` and never a second escape.
 *
 *  A SEGMENT NEVER HOLDS A `/`. An encoded slash (`%2F`) is refused here, not
 *  decoded into the segment and not turned into a separator: an app that
 *  routes on `segments[0] === 'admin'` or joins a segment into a key must be
 *  able to trust that a segment is one segment. A value that can hold a slash
 *  belongs in the query string. */
export function appPathSegments(rawPathname: string): { ok: true; segments: string[] } | { ok: false; error: string } {
  const raw = rawPathname.split('/').filter(Boolean).slice(3)
  const segments: string[] = []
  for (const part of raw) {
    let decoded: string
    try {
      decoded = decodeURIComponent(part)
    } catch {
      return { ok: false, error: 'malformed path: a percent-escape does not decode' }
    }
    if (decoded.includes('/')) {
      return { ok: false, error: 'an encoded "/" (%2F) is not allowed in an app path; send the value in the query string' }
    }
    segments.push(decoded)
  }
  return { ok: true, segments }
}

/** The left-most value of a forwarded header: a proxy chain comma-joins them,
 *  and the first is what the client hit (the convention the api's own origin
 *  reader and client_ip follow). */
const forwarded = (request: Request, name: string): string | null => {
  const v = request.headers.get(name)?.split(',')[0]?.trim()
  return v ? v : null
}

/** The origin a person reaches this instance at — the base for any absolute
 *  link an app builds. The api's own rule (talaria-google-oauth
 *  `resolve_origin`): AUTH_PUBLIC_URL when the operator set it; else the
 *  proto and host the proxy in front stated; else the request's own. The
 *  request URL alone is wrong behind TLS termination: this process is reached
 *  over plain http. A forwarded value that does not make an http(s) origin is
 *  ignored rather than trusted. */
export function appOrigin(request: Request, publicUrl: string | null = getAuthConfig().publicUrl): string {
  const own = new URL(request.url)
  const candidates: string[] = []
  if (publicUrl) candidates.push(publicUrl)
  const proto = forwarded(request, 'x-forwarded-proto') ?? own.protocol.replace(/:$/, '')
  const host = forwarded(request, 'x-forwarded-host') ?? request.headers.get('host') ?? own.host
  candidates.push(`${proto}://${host}`)
  for (const c of candidates) {
    try {
      const u = new URL(c)
      if ((u.protocol === 'http:' || u.protocol === 'https:') && u.host) return u.origin
    } catch {
      // not an origin; try the next
    }
  }
  return own.origin
}

/** What `ctx.api` takes besides the path. `body` that is not already a
 *  string, FormData, Blob, URLSearchParams or bytes is sent as JSON. */
export interface PlatformApiInit {
  method?: string
  body?: unknown
  headers?: HeadersInit
  /** Your own signal: it aborts the call, a write included. A read also
   *  follows the caller's (`ctx.signal`); a write does not, so pass
   *  `ctx.signal` here only to cut a write when the caller leaves. */
  signal?: AbortSignal
  /** Deadline to the response's headers, in ms. Reads default to 30s, the
   *  same ceiling the proxy puts on a browser's reads; writes have none, as
   *  there (an action may wait on docker for minutes). The body is never
   *  timed. */
  timeoutMs?: number
}

export type PlatformApi = <T = unknown>(path: string, init?: PlatformApiInit) => Promise<T>

const READ_DEADLINE_MS = 30_000

/** The prefixes `ctx.api` refuses: the routes this host answers itself (an
 *  app's server is reached from the client, not from another app's server)
 *  and the service-to-service prefix, which answers only the host's own
 *  credential. */
const NOT_THE_PLATFORM = [/^\/api\/apps\//, /^\/api\/app-host\//, /^\/api\/mcp\/gw\/app-/, /^\/api\/internal(\/|$)/]

const isRawBody = (b: unknown): b is BodyInit =>
  typeof b === 'string' ||
  b instanceof FormData ||
  b instanceof Blob ||
  b instanceof URLSearchParams ||
  b instanceof ArrayBuffer ||
  ArrayBuffer.isView(b)

/** The server's error sentence: `{ error: "…" }`, or the proxy's
 *  `{ error: { message } }`. */
function errorSentence(body: unknown, status: number): string {
  if (body && typeof body === 'object' && 'error' in body) {
    const e = (body as { error: unknown }).error
    if (typeof e === 'string' && e) return e
    if (e && typeof e === 'object' && typeof (e as { message?: unknown }).message === 'string') return (e as { message: string }).message
  }
  return `request failed (${status})`
}

/** `ctx.api(path, init)`: the platform API as the CALLER. The request goes
 *  straight to the Rust api (the hop the browser's own requests take through
 *  the proxy) carrying the caller's session cookie and nothing else of
 *  theirs, so every permission and ACL applies exactly as in the core UI: an
 *  app can do no more than the person using it.
 *
 *  Resolves to the parsed JSON (null for an empty body; the text when the
 *  answer is not JSON). A non-2xx answer throws an `HttpError` carrying the
 *  status and the server's own sentence. */
export function platformApi(request: Request, apiBase: () => string | undefined = rustApiUrl): PlatformApi {
  const incoming = new URL(request.url)
  return async <T>(path: string, init: PlatformApiInit = {}): Promise<T> => {
    const base = apiBase()
    if (base === undefined) throw new HttpError(503, 'the platform API is stood down on this host')
    if (typeof path !== 'string' || !path.startsWith('/api/')) throw new TypeError('ctx.api takes a path that starts with /api/')
    const target = new URL(path, base)
    if (target.origin !== new URL(base).origin || !target.pathname.startsWith('/api/')) {
      throw new TypeError('ctx.api reaches the platform API only: the path must stay under /api/')
    }
    if (NOT_THE_PLATFORM.some((r) => r.test(target.pathname))) {
      throw new TypeError(`ctx.api reaches the platform API; ${target.pathname} is not one of its routes`)
    }

    const method = (init.method ?? 'GET').toUpperCase()
    const headers = new Headers(init.headers)
    // The caller's session, and only that: their cookie is the credential.
    // An app's own cookie or bearer header would make this call someone
    // else's, which is what plain fetch is for.
    headers.delete('cookie')
    headers.delete('authorization')
    const cookie = request.headers.get('cookie')
    if (cookie) headers.set('cookie', cookie)
    if (!headers.has('accept')) headers.set('accept', 'application/json')
    // The origin the caller hit, as the proxy states it for the browser's
    // own requests, so origin-derived answers (links, redirects) agree.
    headers.set('x-forwarded-proto', forwarded(request, 'x-forwarded-proto') ?? incoming.protocol.replace(/:$/, ''))
    headers.set('x-forwarded-host', forwarded(request, 'x-forwarded-host') ?? incoming.host)

    let body: BodyInit | undefined
    if (init.body !== undefined && init.body !== null) {
      if (isRawBody(init.body)) body = init.body
      else {
        body = JSON.stringify(init.body)
        if (!headers.has('content-type')) headers.set('content-type', 'application/json')
      }
    }

    // The proxy's rule for the browser's own requests (rust-proxy.ts): the
    // caller leaving cuts a READ, since nobody is waiting for it, and never a
    // WRITE — a task, a fleet verb or an install half done because a tab
    // closed is worse than one finished for nobody. The app's own signal is
    // the app's choice and cuts either. The deadline guards only the wait for
    // the response's headers (cleared when they land), reads by default.
    const read = method === 'GET' || method === 'HEAD'
    const deadline = new AbortController()
    const signals: AbortSignal[] = [deadline.signal]
    if (read && request.signal) signals.push(request.signal)
    if (init.signal) signals.push(init.signal)
    const timeoutMs = init.timeoutMs ?? (read ? READ_DEADLINE_MS : undefined)
    const timer =
      timeoutMs === undefined
        ? undefined
        : setTimeout(() => deadline.abort(new DOMException(`the platform API did not answer in ${timeoutMs}ms`, 'TimeoutError')), timeoutMs)
    let res: Response
    try {
      res = await fetch(target, { method, headers, body, signal: AbortSignal.any(signals), redirect: 'manual' })
    } finally {
      if (timer !== undefined) clearTimeout(timer)
    }
    const text = await res.text()
    let parsed: unknown = text
    if (text === '') parsed = null
    else if ((res.headers.get('content-type') ?? '').includes('json')) {
      try {
        parsed = JSON.parse(text)
      } catch {
        if (res.ok) throw new HttpError(502, 'the platform API sent JSON that does not parse')
      }
    }
    if (!res.ok) throw new HttpError(res.status, errorSentence(parsed, res.status))
    return parsed as T
  }
}

/** `ctx.secret(name)`: a workspace secret's value, for this app's server
 *  code. `name` is the doc (`crm`) or the doc and an entry (`crm.api_key`) —
 *  the handle grammar without the «secret:…» marks. Resolves to null when the
 *  secret does not resolve for this app; the reason goes to the host log,
 *  never to the caller (the rule is `resolveAppSecret`). `userId` is the
 *  person whose request reads it, recorded on the spend; null for a trigger's
 *  run, which has no person behind it. */
export function appSecretReader(slug: string, userId: string | null): (name: string) => Promise<string | null> {
  return async (name: string) => {
    const r = await resolveAppSecret(slug, name, userId)
    if (r.ok) return r.value
    console.warn(`[apps:${slug}] ctx.secret("${String(name).slice(0, 80)}") did not resolve: ${r.reason}`)
    return null
  }
}
