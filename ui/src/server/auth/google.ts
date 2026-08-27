// Google LOGIN — the policy half. The OAuth mechanics (endpoints, code
// exchange, userinfo) live once in google/oauth.ts, the leaf both flows
// build on; this file decides what a verified Google identity may do:
// the Workspace domain gate, and the shaping into a Talaria identity.
// (Tokens are hand-rolled, no SDK — straight from Google's token endpoint
// over TLS, the profile from userinfo so we never verify a JWT locally.)

import { emailDomainOf } from '../google/aliasing'
import { AUTH_ENDPOINT, exchangeGoogleTokens, resolveOrigin } from '../google/oauth'
import { resolveGoogleClient, type GoogleClient } from '../google/client-config'
import type { Identity } from '../users'

export type LoginResult = Identity & { provider: 'google' }

// RE-EXPORTED, not re-declared: this was resolveOrigin's birthplace, and
// admin.google-client.ts (plus this file's own redirect URI) keeps the import
// it already had.
export { resolveOrigin } from '../google/oauth'

export const googleRedirectUri = (request: Request) => `${resolveOrigin(request)}/api/auth/google/callback`

export async function googleAuthUrl(redirectUri: string, state: string): Promise<string> {
  const cfg = (await resolveGoogleClient())!
  const params = new URLSearchParams({
    client_id: cfg.clientId,
    redirect_uri: redirectUri,
    response_type: 'code',
    scope: 'openid email profile',
    state,
    access_type: 'online',
    prompt: 'select_account',
  })
  if (cfg.hd) params.set('hd', cfg.hd)
  return `${AUTH_ENDPOINT}?${params.toString()}`
}

/** May this Google identity sign in, given the connected org account's email?
 *  A Talaria wired to a Google Workspace is FOR that workspace's people: once
 *  the org account is connected, Google sign-in is domain-members-only — even
 *  an invited outside address is refused (invite them a password instead).
 *  No org connection (null orgEmail) gates nothing: a fresh install's logins
 *  are governed by the allow-list/invite doors alone, as before. */
export function orgGoogleLoginAllowed(orgEmail: string | null | undefined, loginEmail: string | null | undefined): boolean {
  if (!orgEmail) return true
  const domain = emailDomainOf(orgEmail)
  if (!domain) return true // a connected row with no usable email cannot gate anything
  return emailDomainOf(loginEmail) === domain
}

/** Exchange the auth code and resolve the Google identity. The client comes
 *  from the Admin UI record or the env fallback — login itself stays gated by
 *  AUTH_GOOGLE_ENABLED (see `googleLoginEnabled`). */
export async function exchangeGoogleCode(code: string, redirectUri: string): Promise<LoginResult> {
  const cfg = (await resolveGoogleClient()) as GoogleClient
  const { info } = await exchangeGoogleTokens(code, redirectUri)

  // Enforce the Workspace hosted-domain restriction on the resolved identity too
  // (the `hd` auth param is a hint, not a guarantee).
  if (cfg.hd && info.hd !== cfg.hd) {
    throw new Error(`google account not in required domain ${cfg.hd}`)
  }

  return {
    sub: `google:${info.sub}`,
    email: info.email ?? null,
    name: info.name ?? info.email ?? null,
    picture: info.picture ?? null,
    provider: 'google' as const,
  }
}
