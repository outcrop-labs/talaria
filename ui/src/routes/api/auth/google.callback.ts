import { createFileRoute } from '@tanstack/react-router'
import { getAuthConfig, isEmailAllowed } from '@/server/auth/config'
import { exchangeGoogleCode, googleRedirectUri } from '@/server/auth/google'
import {
  clearStateCookie,
  createSession,
  parseCookies,
  sessionCookie,
  STATE_COOKIE,
} from '@/server/auth/session'
import { upsertUser } from '@/server/users'
import { selfJoinAllowed } from '@/server/org-domains'
import { inviteAllowed, markInviteAccepted } from '@/server/invites'

// Bounce back to the login screen with a machine-readable reason.
function loginError(reason: string): Response {
  return new Response(null, {
    status: 302,
    headers: { Location: `/login?error=${encodeURIComponent(reason)}`, 'Set-Cookie': clearStateCookie() },
  })
}

// GET /api/auth/google/callback → verify state, exchange the code, mint the
// session, and land on the cockpit.
export const Route = createFileRoute('/api/auth/google/callback')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const cfg = getAuthConfig()
        if (!cfg.google.enabled) return loginError('google_disabled')

        const url = new URL(request.url)
        const code = url.searchParams.get('code')
        const state = url.searchParams.get('state')
        const cookieState = parseCookies(request)[STATE_COOKIE]

        if (url.searchParams.get('error')) return loginError('google_denied')
        if (!code || !state || !cookieState || state !== cookieState) {
          return loginError('bad_state')
        }

        let identity
        try {
          identity = await exchangeGoogleCode(code, googleRedirectUri(request))
        } catch (err) {
          if (import.meta.env.DEV) console.error('[auth/google] callback failed:', err)
          return loginError('exchange_failed')
        }

        // Three doors in: the env allow-list, SELF-JOIN (verified email on a
        // DNS-verified org domain), or a live INVITE for this address.
        const invited = await inviteAllowed(identity.email)
        if (!isEmailAllowed(identity.email, cfg) && !(await selfJoinAllowed(identity.email)) && !invited) {
          return loginError('not_allowed')
        }

        const user = await upsertUser(identity)
        if (invited && identity.email) void markInviteAccepted(identity.email, user.id)
        const sid = await createSession({ ...user, provider: identity.provider })
        const headers = new Headers({ Location: '/' })
        // Set the session and drop the one-shot state cookie.
        headers.append('Set-Cookie', sessionCookie(sid))
        headers.append('Set-Cookie', clearStateCookie())
        return new Response(null, { status: 302, headers })
      },
    },
  },
})
