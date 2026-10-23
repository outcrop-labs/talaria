// The caller's session as the Rust api resolves it: `GET /api/auth/session`
// with the caller's own cookie. The user, the core views they may not reach
// (person + team grants + the agent-manager grant), the app views they hold
// and their effective permissions, read fresh each time. Kept apart from
// app-access.ts so a guard (api-guard.ts `requireView`) can ask without
// pulling the app registry in.
import type { SessionUser } from './auth/session'
import { rustApiUrl } from './rust-proxy'

/** What `GET /api/auth/session` answers for the caller. */
export interface RustSession {
  user: SessionUser | null
  deniedViews: string[]
  /** The app views the caller holds (`ViewGrants::app_grants`). The app
   *  gates admit on this list; empty for an admin, who holds every app by
   *  role. `null` when the api could not read the enabled apps
   *  (`appsUnreadable`): the caller holds no app, and the app doors answer
   *  503. The core `deniedViews` and `perms` are whole either way. */
  appGrants: string[] | null
  /** The api could not read the enabled apps, so it withheld `appGrants`. */
  appsUnreadable: boolean
  perms: string[]
}

/** The session read must answer; it is one loopback hop on the same box. */
const SESSION_DEADLINE_MS = 10_000

/** One of the session's lists, or a throw. A missing or mistyped list is a
 *  broken answer, not an empty one: `deniedViews: []` read off a missing key
 *  is "no denials", and that guess is an authorization answer. */
function stringList(body: Record<string, unknown>, key: string): string[] {
  const v = body[key]
  if (!Array.isArray(v) || !v.every((x): x is string => typeof x === 'string')) {
    throw new Error(`session read answered no "${key}" list`)
  }
  return v
}

/** The Rust session for this request's cookie. Throws when the api cannot
 *  answer: an unreadable session is never read as "signed out", as "no
 *  denials" or as "no grants" — each would be a guess at an authorization
 *  answer. */
export async function rustSession(request: Request, fetchImpl: typeof fetch = fetch): Promise<RustSession> {
  const base = rustApiUrl()
  if (base === undefined) throw new Error('the Rust api is stood down (TALARIA_RUST_API_URL=off)')
  const headers = new Headers({ accept: 'application/json' })
  const cookie = request.headers.get('cookie')
  if (cookie) headers.set('cookie', cookie)
  const res = await fetchImpl(new URL('/api/auth/session', base), {
    headers,
    signal: AbortSignal.timeout(SESSION_DEADLINE_MS),
  })
  if (!res.ok) throw new Error(`session read answered ${res.status}`)
  const raw: unknown = await res.json()
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('session read answered no object')
  const body = raw as { user?: (SessionUser & Record<string, unknown>) | null } & Record<string, unknown>
  // The same contract the SPA holds this route to (lib/session.ts): `user:
  // null` is "nobody signed in"; a body with no `user` at all is broken.
  if (body.user === undefined) throw new Error('session read answered no "user" field')
  const deniedViews = stringList(body, 'deniedViews')
  // `appsUnreadable: true` withholds the grant list (`appGrants: null`); any
  // other answer must carry the list. A missing flag is an older api's
  // answer, which always carried it.
  const flag = body.appsUnreadable
  if (flag !== undefined && typeof flag !== 'boolean') throw new Error('session read answered a non-boolean "appsUnreadable"')
  const appsUnreadable = flag === true
  const appGrants = appsUnreadable ? null : stringList(body, 'appGrants')
  const perms = stringList(body, 'perms')
  const u = body.user
  return {
    // The documented SessionUser shape, not whatever else the wire carries
    // (status emoji and text): this object is handed to app code.
    user: u
      ? { id: u.id, sub: u.sub, email: u.email ?? null, name: u.name ?? null, picture: u.picture ?? null, provider: u.provider, role: u.role }
      : null,
    deniedViews,
    appGrants,
    appsUnreadable,
    perms,
  }
}
