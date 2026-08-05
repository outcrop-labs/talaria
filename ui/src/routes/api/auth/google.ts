import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getAuthConfig } from '@/server/auth/config'
import { googleAuthUrl, googleRedirectUri } from '@/server/auth/google'
import { randomToken, stateCookie } from '@/server/auth/session'

// GET /api/auth/google → begin the OAuth dance: set a signed state cookie and
// 302 to Google's consent screen.
export const Route = defineApi('/api/auth/google', {
  GET: async ({ request }) => {
    const cfg = getAuthConfig()
    if (!cfg.google.enabled) {
      return json({ error: 'Google login is disabled' }, { status: 400 })
    }

    const state = randomToken()
    const url = googleAuthUrl(googleRedirectUri(request), state)
    return new Response(null, {
      status: 302,
      headers: { Location: url, 'Set-Cookie': stateCookie(state) },
    })
  },
})
