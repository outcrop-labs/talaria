import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { getAuthConfig, isEmailAllowed } from '@/server/auth/config'
import { verifyPasswordLogin } from '@/server/auth/password'
import { createSession, sessionCookie } from '@/server/auth/session'
import { upsertUser } from '@/server/users'
import { clientIp, rateLimit, rateLimitReset } from '@/server/rate-limit'

const Body = z.object({
  username: z.string().min(1).max(200),
  password: z.string().min(1).max(1000),
})

// Brute-force brake. Two counters, because either one alone has a hole:
//   • per USERNAME — the one that actually bounds guessing at a given account,
//     and the only one an attacker can't sidestep by changing headers or hosts.
//   • per IP — catches spraying across many usernames, but only counts when a
//     trusted proxy is configured (see clientIp); otherwise every request looks
//     like the same 'direct' client, which is the safe direction.
// Both live in Redis, so they survive a restart and hold across instances — the
// old per-process Map did neither, and keyed off an unvalidated
// X-Forwarded-For, so `curl -H 'X-Forwarded-For: 1.2.3.<n>'` reset it at will.
const USER_LIMIT = 10
const IP_LIMIT = 30
const WINDOW_SECONDS = 15 * 60

// POST /api/auth/password { username, password } → sets the session cookie.
export const Route = defineApi('/api/auth/password', {
  POST: async ({ request }) => {
    const cfg = getAuthConfig()
    if (!cfg.password.enabled) {
      return json({ ok: false, error: 'Password login is disabled' }, { status: 400 })
    }

    // Parse first: the username is what the primary counter keys on.
    const parsed = Body.safeParse(await request.json().catch(() => null))
    if (!parsed.success) {
      return json({ ok: false, error: 'Invalid request' }, { status: 400 })
    }

    const userKey = `login:user:${parsed.data.username.trim().toLowerCase()}`
    const ipKey = `login:ip:${clientIp(request)}`
    const [byUser, byIp] = await Promise.all([
      rateLimit(userKey, USER_LIMIT, WINDOW_SECONDS),
      rateLimit(ipKey, IP_LIMIT, WINDOW_SECONDS),
    ])
    const limited = !byUser.ok ? byUser : !byIp.ok ? byIp : null
    if (limited) {
      return json(
        { ok: false, error: 'Too many attempts, try again shortly' },
        { status: 429, headers: { 'Retry-After': String(limited.retryAfter) } },
      )
    }

    const identity = verifyPasswordLogin(parsed.data.username, parsed.data.password)
    if (!identity || !isEmailAllowed(identity.email, cfg)) {
      // Slow the failure path a touch to blunt brute force.
      await new Promise((r) => setTimeout(r, 400))
      return json({ ok: false, error: 'Invalid credentials' }, { status: 401 })
    }

    // A real login clears the budget so a fat-fingered morning doesn't lock
    // someone out for the rest of the window.
    await rateLimitReset(userKey)
    const user = await upsertUser(identity)
    const sid = await createSession({ ...user, provider: identity.provider })
    return json({ ok: true, user }, { headers: { 'Set-Cookie': sessionCookie(sid) } })
  },
})
