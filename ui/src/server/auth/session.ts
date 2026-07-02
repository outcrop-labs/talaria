// Sessions — Redis-backed. The cookie carries only an opaque session id; the
// user record lives in Redis under `sess:<sid>` with a TTL. Logout deletes it.
// (OAuth state stays a short signed-free double-submit cookie — no server state.)

import { randomBytes } from 'node:crypto'
import { getRedis } from '../db/redis'
import type { ProviderId } from './config'
import type { Role } from '../users'

export const SESSION_COOKIE = 'talaria_session'
export const STATE_COOKIE = 'talaria_oauth_state'
const SESSION_TTL_SECONDS = 60 * 60 * 24 * 7 // 7 days

export interface SessionUser {
  id: string
  sub: string
  email: string | null
  name: string | null
  picture: string | null
  provider: ProviderId
  role: Role
}

const key = (sid: string) => `sess:${sid}`

/** Create a session in Redis; returns the opaque session id for the cookie. */
export async function createSession(user: SessionUser): Promise<string> {
  const sid = randomToken()
  await getRedis().set(key(sid), JSON.stringify(user), 'EX', SESSION_TTL_SECONDS)
  return sid
}

/** Read + resolve the current user from the request's session cookie. */
export async function getSessionUser(request: Request): Promise<SessionUser | null> {
  const sid = parseCookies(request)[SESSION_COOKIE]
  if (!sid) return null
  const raw = await getRedis().get(key(sid))
  if (!raw) return null
  try {
    return JSON.parse(raw) as SessionUser
  } catch {
    return null
  }
}

/** Merge changes into the live session (e.g. a display-name edit) so the
 *  updated identity shows without re-login. Keeps the existing TTL. */
export async function updateSessionUser(request: Request, patch: Partial<SessionUser>): Promise<SessionUser | null> {
  const sid = parseCookies(request)[SESSION_COOKIE]
  if (!sid) return null
  const raw = await getRedis().get(key(sid))
  if (!raw) return null
  const next = { ...(JSON.parse(raw) as SessionUser), ...patch }
  await getRedis().set(key(sid), JSON.stringify(next), 'KEEPTTL')
  return next
}

/** Delete the session behind the request's cookie (logout). */
export async function destroySession(request: Request): Promise<void> {
  const sid = parseCookies(request)[SESSION_COOKIE]
  if (sid) await getRedis().del(key(sid))
}

// ── Cookie helpers ───────────────────────────────────────────────────────────

export function parseCookies(request: Request): Record<string, string> {
  const header = request.headers.get('cookie') ?? ''
  const out: Record<string, string> = {}
  for (const part of header.split(';')) {
    const idx = part.indexOf('=')
    if (idx === -1) continue
    const k = part.slice(0, idx).trim()
    if (k) out[k] = decodeURIComponent(part.slice(idx + 1).trim())
  }
  return out
}

function cookieString(name: string, value: string, maxAge: number): string {
  // Secure by default in production, unless COOKIE_SECURE opts out (browsers drop
  // Secure cookies over plain http://, which breaks login on LAN deployments).
  const override = (process.env.COOKIE_SECURE ?? '').trim().toLowerCase()
  const insecure = override === '0' || override === 'false' || override === 'no'
  const secure = !insecure && process.env.NODE_ENV === 'production' ? '; Secure' : ''
  return `${name}=${encodeURIComponent(value)}; Path=/; HttpOnly; SameSite=Lax; Max-Age=${maxAge}${secure}`
}

export const sessionCookie = (sid: string) => cookieString(SESSION_COOKIE, sid, SESSION_TTL_SECONDS)
export const clearSessionCookie = () => cookieString(SESSION_COOKIE, '', 0)
export const stateCookie = (value: string) => cookieString(STATE_COOKIE, value, 600) // 10 min
export const clearStateCookie = () => cookieString(STATE_COOKIE, '', 0)

/** Random URL-safe token for session ids / OAuth state. */
export const randomToken = (bytes = 32) => randomBytes(bytes).toString('base64url')
