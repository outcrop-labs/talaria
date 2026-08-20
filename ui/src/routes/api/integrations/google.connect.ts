import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSessionUser } from '@/server/auth/session'
import { randomToken, stateCookie } from '@/server/auth/session'
import { googleConnectRedirectUri, googleConnectUrl, googleIntegrationEnabled } from '@/server/google/oauth'

// GET /api/integrations/google/connect → begin the offline-access consent dance.
// Requires an authenticated session (we bind the connection to this user).
export const Route = defineApi('/api/integrations/google/connect', {
  GET: async ({ request }) => {
    const user = await getSessionUser(request)
    if (!user) return new Response(null, { status: 302, headers: { Location: '/login' } })
    if (!(await googleIntegrationEnabled())) {
      return json({ error: 'Google integration is not configured' }, { status: 400 })
    }
    const state = randomToken()
    const url = await googleConnectUrl(googleConnectRedirectUri(request), state)
    return new Response(null, {
      status: 302,
      headers: { Location: url, 'Set-Cookie': stateCookie(state) },
    })
  },
})
