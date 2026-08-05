import { defineApi } from '@/server/api-route'
import { getSessionUser } from '@/server/auth/session'
import { clearStateCookie, parseCookies, STATE_COOKIE } from '@/server/auth/session'
import { completeGoogleConnect, googleConnectRedirectUri, googleIntegrationEnabled } from '@/server/google/oauth'

// Land back on the integrations settings with a status flag.
function back(status: string): Response {
  return new Response(null, {
    status: 302,
    headers: { Location: `/settings?google=${encodeURIComponent(status)}`, 'Set-Cookie': clearStateCookie() },
  })
}

// GET /api/integrations/google/callback → verify state, exchange the code for an
// offline refresh token, and store the connection for the signed-in user.
export const Route = defineApi('/api/integrations/google/callback', {
  GET: async ({ request }) => {
    if (!googleIntegrationEnabled()) return back('disabled')
    const user = await getSessionUser(request)
    if (!user) return new Response(null, { status: 302, headers: { Location: '/login' } })

    const url = new URL(request.url)
    const code = url.searchParams.get('code')
    const state = url.searchParams.get('state')
    const cookieState = parseCookies(request)[STATE_COOKIE]

    if (url.searchParams.get('error')) return back('denied')
    if (!code || !state || !cookieState || state !== cookieState) return back('bad_state')

    try {
      await completeGoogleConnect(user.id, code, googleConnectRedirectUri(request), Date.now())
    } catch (err) {
      if (import.meta.env.DEV) console.error('[integrations/google] connect failed:', err)
      return back('exchange_failed')
    }
    return back('connected')
  },
})
