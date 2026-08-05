import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { clearSessionCookie, destroySession } from '@/server/auth/session'

// POST /api/auth/logout → delete the Redis session + clear the cookie.
export const Route = defineApi('/api/auth/logout', {
  POST: async ({ request }) => {
    await destroySession(request)
    return json({ ok: true }, { headers: { 'Set-Cookie': clearSessionCookie() } })
  },
})
