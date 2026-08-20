// Google OAuth 2.0 (Authorization Code flow), hand-rolled — no SDK.
// Tokens come straight from Google's token endpoint over TLS, then we read the
// profile from the userinfo endpoint (so we never have to verify a JWT locally).

import { getAuthConfig } from './config'
import { resolveGoogleClient, type GoogleClient } from '../google/client-config'
import type { Identity } from '../users'

export type LoginResult = Identity & { provider: 'google' }

const AUTH_ENDPOINT = 'https://accounts.google.com/o/oauth2/v2/auth'
const TOKEN_ENDPOINT = 'https://oauth2.googleapis.com/token'
const USERINFO_ENDPOINT = 'https://openidconnect.googleapis.com/v1/userinfo'

/** External origin for redirect URIs: AUTH_PUBLIC_URL, else derived from the request. */
export function resolveOrigin(request: Request): string {
  const configured = getAuthConfig().publicUrl
  if (configured) return configured
  const url = new URL(request.url)
  const proto = request.headers.get('x-forwarded-proto') ?? url.protocol.replace(':', '')
  const host = request.headers.get('x-forwarded-host') ?? request.headers.get('host') ?? url.host
  return `${proto}://${host}`
}

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

interface GoogleUserInfo {
  sub: string
  email?: string
  email_verified?: boolean
  name?: string
  picture?: string
  hd?: string
}

/** Exchange the auth code and resolve the Google identity. The client comes
 *  from the Admin UI record or the env fallback — login itself stays gated by
 *  AUTH_GOOGLE_ENABLED (see `googleLoginEnabled`). */
export async function exchangeGoogleCode(code: string, redirectUri: string): Promise<LoginResult> {
  const cfg = (await resolveGoogleClient()) as GoogleClient

  const tokenRes = await fetch(TOKEN_ENDPOINT, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      code,
      client_id: cfg.clientId,
      client_secret: cfg.clientSecret,
      redirect_uri: redirectUri,
      grant_type: 'authorization_code',
    }),
  })
  if (!tokenRes.ok) {
    throw new Error(`google token exchange failed: ${tokenRes.status} ${await tokenRes.text()}`)
  }
  const tokens = (await tokenRes.json()) as { access_token?: string }
  if (!tokens.access_token) throw new Error('google token exchange: no access_token')

  const infoRes = await fetch(USERINFO_ENDPOINT, {
    headers: { Authorization: `Bearer ${tokens.access_token}` },
  })
  if (!infoRes.ok) {
    throw new Error(`google userinfo failed: ${infoRes.status} ${await infoRes.text()}`)
  }
  const info = (await infoRes.json()) as GoogleUserInfo

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
