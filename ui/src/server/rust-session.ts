// The caller's session as the Rust api resolves it: `GET /api/auth/session`
// with the caller's own cookie. The user, the views they may not reach
// (person + team grants + the agent-manager grant) and their effective
// permissions, read fresh each time. Kept apart from app-access.ts so a guard
// (api-guard.ts `requireView`) can ask without pulling the app registry in.
import type { SessionUser } from './auth/session'
import { rustApiUrl } from './rust-proxy'

/** What `GET /api/auth/session` answers for the caller. */
export interface RustSession {
  user: SessionUser | null
  deniedViews: string[]
  perms: string[]
}

/** The session read must answer; it is one loopback hop on the same box. */
const SESSION_DEADLINE_MS = 10_000

/** The Rust session for this request's cookie. Throws when the api cannot
 *  answer: an unreadable session is never read as "signed out" or as "no
 *  denials" — either would be a guess at an authorization answer. */
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
  const body = (await res.json()) as { user?: (SessionUser & Record<string, unknown>) | null; deniedViews?: unknown; perms?: unknown }
  // The same contract the SPA holds this route to (lib/session.ts): `user:
  // null` is "nobody signed in"; a body with no `user` at all is broken.
  if (body.user === undefined) throw new Error('session read answered no "user" field')
  const stringList = (v: unknown): string[] => (Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [])
  const u = body.user
  return {
    // The documented SessionUser shape, not whatever else the wire carries
    // (status emoji and text): this object is handed to app code.
    user: u
      ? { id: u.id, sub: u.sub, email: u.email ?? null, name: u.name ?? null, picture: u.picture ?? null, provider: u.provider, role: u.role }
      : null,
    deniedViews: stringList(body.deniedViews),
    perms: stringList(body.perms),
  }
}
