import { defineApi } from '@/server/api-route'
import { getAuthConfig, isEmailAllowed } from '@/server/auth/config'
import { claimAdmin, instanceClaimable } from '@/server/auth/claim'
import { logAudit } from '@/server/audit'
import { googleLoginEnabled } from '@/server/google/client-config'
import { exchangeGoogleCode, googleRedirectUri, orgGoogleLoginAllowed } from '@/server/auth/google'
import {
  clearStateCookie,
  createSession,
  parseCookies,
  sessionCookie,
  stateMatches,
  STATE_COOKIE,
} from '@/server/auth/session'
import { upsertUser } from '@/server/users'
import { selfJoinAllowed } from '@/server/org-domains'
import { inviteAllowed, markInviteAccepted } from '@/server/invites'
import { emailDomainOf } from '@/server/google/aliasing'
import { getOrgConnectionStatus } from '@/server/google/org-connection'

// Bounce back to the login screen with a machine-readable reason. Extra params
// ride along (the org-domain refusal carries WHICH domain to name).
function loginError(reason: string, extra: Record<string, string> = {}): Response {
  const params = new URLSearchParams({ error: reason, ...extra })
  return new Response(null, {
    status: 302,
    headers: { Location: `/login?${params.toString()}`, 'Set-Cookie': clearStateCookie() },
  })
}

// GET /api/auth/google/callback → verify state, exchange the code, mint the
// session, and land on the cockpit.
export const Route = defineApi('/api/auth/google/callback', {
  GET: async ({ request }) => {
    const cfg = getAuthConfig()
    if (!(await googleLoginEnabled())) return loginError('google_disabled')

    const url = new URL(request.url)
    const code = url.searchParams.get('code')
    const state = url.searchParams.get('state')
    const cookieState = parseCookies(request)[STATE_COOKIE]

    if (url.searchParams.get('error')) return loginError('google_denied')
    if (!code || !state || !cookieState || !stateMatches(state, cookieState)) {
      return loginError('bad_state')
    }

    let identity
    try {
      identity = await exchangeGoogleCode(code, googleRedirectUri(request))
    } catch (err) {
      if (import.meta.env.DEV) console.error('[auth/google] callback failed:', err)
      return loginError('exchange_failed')
    }

    // An unclaimed instance (zero admins) hands the FIRST Google identity the
    // keys — no doors apply, by design: nobody exists yet to have invited them.
    // A lost race (someone claimed between the check and the lock) falls
    // through to the normal doors for this sign-in. The org-domain gate below
    // cannot bite here in practice — org connect is admin-only, so an
    // unclaimed instance has no connection — but the claim precedes it anyway.
    if (await instanceClaimable()) {
      const claimed = await claimAdmin(identity)
      if (claimed) {
        void logAudit({
          actor: identity.email ?? claimed.id,
          action: 'auth.claim',
          targetType: 'user',
          targetId: claimed.id,
          after: { email: identity.email, role: claimed.role, provider: 'google' },
        })
        const sid = await createSession({ ...claimed, provider: identity.provider })
        const headers = new Headers({ Location: '/' })
        headers.append('Set-Cookie', sessionCookie(sid))
        headers.append('Set-Cookie', clearStateCookie())
        return new Response(null, { status: 302, headers })
      }
    }

    // The org account's domain is the outer gate. Once a Talaria is wired to
    // a Google Workspace, Google sign-in is for that workspace's people —
    // checked BEFORE the doors below because "only this org" is a property of
    // the install, not a membership policy an invite can override. The
    // connected flag (a live refresh token) is part of the anchor: a dead
    // leftover row must not lock every human out.
    const org = await getOrgConnectionStatus()
    if (!orgGoogleLoginAllowed(org.connected ? org.email : null, identity.email)) {
      return loginError('org_domain', { domain: emailDomainOf(org.email) ?? '' })
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
})
