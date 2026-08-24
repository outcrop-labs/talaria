import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { getSessionUser } from '@/server/auth/session'
import { randomToken, stateCookie } from '@/server/auth/session'
import { googleConnectUrl, googleIntegrationEnabled, googleOrgConnectRedirectUri, ORG_CONNECT_SCOPES } from '@/server/google/oauth'

// GET /api/integrations/google/org/connect → admin begins connecting the shared
// org Google account (offline access). Wider scopes than the per-user flow:
// the org account is the one that provisions the shared calendar + Drive.
export const Route = defineApi('/api/integrations/google/org/connect', {
  GET: async ({ request }) => {
    const user = await getSessionUser(request)
    if (!user) return new Response(null, { status: 302, headers: { Location: '/login' } })
    if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
    if (!(await googleIntegrationEnabled())) return json({ error: 'Google integration is not configured' }, { status: 400 })
    const state = randomToken()
    const url = await googleConnectUrl(googleOrgConnectRedirectUri(request), state, ORG_CONNECT_SCOPES)
    return new Response(null, { status: 302, headers: { Location: url, 'Set-Cookie': stateCookie(state) } })
  },
})
