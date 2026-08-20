import { defineApi } from '@/server/api-route'
import { getSessionUser } from '@/server/auth/session'
import { clearStateCookie, parseCookies, STATE_COOKIE } from '@/server/auth/session'
import { completeGoogleOrgConnect, googleIntegrationEnabled, googleOrgConnectRedirectUri } from '@/server/google/oauth'

function back(status: string): Response {
  return new Response(null, {
    status: 302,
    headers: { Location: `/admin?googleOrg=${encodeURIComponent(status)}`, 'Set-Cookie': clearStateCookie() },
  })
}

// GET /api/integrations/google/org/callback → store the shared org connection.
export const Route = defineApi('/api/integrations/google/org/callback', {
  GET: async ({ request }) => {
    if (!(await googleIntegrationEnabled())) return back('disabled')
    const user = await getSessionUser(request)
    if (!user) return new Response(null, { status: 302, headers: { Location: '/login' } })
    if (user.role !== 'admin') return back('forbidden')

    const url = new URL(request.url)
    const code = url.searchParams.get('code')
    const state = url.searchParams.get('state')
    const cookieState = parseCookies(request)[STATE_COOKIE]

    if (url.searchParams.get('error')) return back('denied')
    if (!code || !state || !cookieState || state !== cookieState) return back('bad_state')

    try {
      await completeGoogleOrgConnect(user.id, code, googleOrgConnectRedirectUri(request), Date.now())
    } catch (err) {
      if (import.meta.env.DEV) console.error('[integrations/google/org] connect failed:', err)
      return back('exchange_failed')
    }
    return back('connected')
  },
})
