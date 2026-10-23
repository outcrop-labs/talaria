// The app host's own calls to the api — the `/api/internal/*` routes, which
// answer only the root-derived service credential (`internalToken`,
// secretbox.ts; `require_internal` on the Rust side). They exist for the
// moments the host knows something the api must act on and no person is in
// the request: an app's build broke, its catalog changed. The proxy never
// forwards the prefix from outside (rust-proxy.ts), so these are the only
// callers.
//
// Never throws. Every caller here is a side effect of something that already
// happened (the build failed either way), and the api reconciles the same
// state on its own boot — a call that does not land is a warning in the log,
// not a failure to propagate.
import { timingSafeEqual } from 'node:crypto'
import { json } from './http'
import { rustApiUrl } from './rust-proxy'
import { internalToken } from './secretbox'

export const INTERNAL_HEADER = 'x-talaria-internal'

/** Whether `request` carries the root-derived service credential — the api
 *  calling the host back (talaria-apps `AppHost`: an install's build, an
 *  update's stage and activate). Compared in constant time; a host that
 *  cannot derive the token (no root) admits nothing. */
export function isInternalCall(request: Request): boolean {
  const presented = request.headers.get(INTERNAL_HEADER)?.trim() ?? ''
  if (!presented) return false
  let want: string
  try {
    want = internalToken()
  } catch {
    return false
  }
  const a = Buffer.from(presented)
  const b = Buffer.from(want)
  return a.length === b.length && timingSafeEqual(a, b)
}

/** The api, or the 403 to answer — the TS twin of Rust's `require_internal`. */
export function requireInternal(request: Request): Response | null {
  if (isInternalCall(request)) return null
  return json({ error: 'internal route: only the api may call it' }, { status: 403 })
}

/** A reconcile is a few queries and, at worst, a roll that is spawned, not
 *  awaited — it answers in well under this. */
const DEADLINE_MS = 15_000

/** POST one internal route. The parsed JSON answer, or null (logged) when
 *  the api is stood down, unreachable or refuses. */
export async function postInternal(path: string, what: string): Promise<unknown | null> {
  const base = rustApiUrl()
  if (base === undefined) return null // TALARIA_RUST_API_URL=off: no api to ask
  let token: string
  try {
    token = internalToken()
  } catch (e) {
    console.warn(`[internal-api] ${what}: no root secret to sign with (${(e as Error).message})`)
    return null
  }
  try {
    const res = await fetch(`${base}${path}`, {
      method: 'POST',
      headers: { [INTERNAL_HEADER]: token, 'content-type': 'application/json' },
      body: '{}',
      redirect: 'manual',
      signal: AbortSignal.timeout(DEADLINE_MS),
    })
    if (!res.ok) {
      console.warn(`[internal-api] ${what}: the api answered ${res.status}`)
      return null
    }
    return (await res.json()) as unknown
  } catch (e) {
    console.warn(`[internal-api] ${what}: ${(e as Error).message}`)
    return null
  }
}

/** Ask the api to reconcile the registry rows of app-published MCP servers
 *  with the apps (talaria-apps `sync_app_mcp_servers`): copy each serving
 *  app's catalog from its probe, and turn off — rolling its carriers, keeping
 *  every grant — the row of an app that is turned off or failed its build.
 *  Answers the app server names that serve, or null when the call did not
 *  land. */
export async function resyncAppMcpServers(reason: string): Promise<string[] | null> {
  const body = (await postInternal('/api/internal/apps/mcp-sync', `app MCP resync (${reason})`)) as {
    servers?: unknown
  } | null
  if (!body || !Array.isArray(body.servers)) return null
  return body.servers.filter((s): s is string => typeof s === 'string')
}
