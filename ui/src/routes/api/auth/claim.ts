import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody } from '@/server/api-guard'
import { claimAdmin } from '@/server/auth/claim'
import { hashPassword } from '@/server/auth/password'
import { createSession, sessionCookie } from '@/server/auth/session'
import { logAudit } from '@/server/audit'
import { clientIp, rateLimit, rateLimitReset } from '@/server/rate-limit'

const Body = z.object({
  // Stored lowercased + trimmed — the credential's email is its login key.
  email: z.preprocess(
    (v) => (typeof v === 'string' ? v.trim().toLowerCase() : v),
    z.string().email().max(200),
  ),
  password: z.string().min(8).max(1000),
  name: z.string().max(200).optional(),
})

// POST /api/auth/claim { email, password, name? } → the FIRST admin.
//
// Offered only while the instance has zero admins (GET /api/auth/providers →
// claimable); claimAdmin's advisory lock closes the race, so a lost race is a
// 409, never a second admin. Reachable by whoever gets there first on a fresh
// install — by design: whoever deploys, owns (same trust model as the Google
// claim, which needs no form at all).
export const Route = defineApi('/api/auth/claim', {
  POST: async ({ request }) => {
    const parsed = await parseBody(request, Body)
    if (parsed instanceof Response) return parsed

    const ipKey = `claim:ip:${clientIp(request)}`
    const byIp = await rateLimit(ipKey, 10, 15 * 60)
    if (!byIp.ok) {
      return json(
        { error: 'Too many attempts, try again shortly' },
        { status: 429, headers: { 'Retry-After': String(byIp.retryAfter) } },
      )
    }

    const claimed = await claimAdmin(
      {
        sub: `password:${parsed.email}`,
        email: parsed.email,
        name: parsed.name?.trim() || parsed.email,
        picture: null,
      },
      await hashPassword(parsed.password),
    )
    if (!claimed) {
      return json({ error: 'This instance already has an admin — sign in instead.' }, { status: 409 })
    }

    await rateLimitReset(ipKey)
    // No session exists yet, so the actor is the claimed email itself.
    void logAudit({
      actor: parsed.email,
      action: 'auth.claim',
      targetType: 'user',
      targetId: claimed.id,
      after: { email: parsed.email, role: claimed.role, provider: 'password' },
    })
    const sid = await createSession({ ...claimed, provider: 'password' })
    return json({ ok: true, user: claimed }, { headers: { 'Set-Cookie': sessionCookie(sid) } })
  },
})
