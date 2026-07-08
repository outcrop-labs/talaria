// Google connect flow — DISTINCT from google login (server/auth/google.ts).
//
// Login proves who you are (access_type=online, no stored token). Connecting
// grants Talaria durable, offline access to your Drive/Docs (access_type=offline,
// prompt=consent → a refresh token we store encrypted). Separate redirect URI so
// the two flows never cross-wire.

import { getAuthConfig } from '../auth/config'
import { resolveOrigin } from '../auth/google'
import { saveConnection } from './connections'

const AUTH_ENDPOINT = 'https://accounts.google.com/o/oauth2/v2/auth'
const TOKEN_ENDPOINT = 'https://oauth2.googleapis.com/token'
const USERINFO_ENDPOINT = 'https://openidconnect.googleapis.com/v1/userinfo'

// Least-privilege for export/import: drive.file grants access only to files the
// app itself creates or the user explicitly opens with it — not the whole Drive.
export const DRIVE_SCOPES = [
  'openid',
  'email',
  'https://www.googleapis.com/auth/drive.file',
]

/** Whether the Google integration can run at all (same client as login). */
export function googleIntegrationEnabled(): boolean {
  return getAuthConfig().google.enabled
}

export const googleConnectRedirectUri = (request: Request) =>
  `${resolveOrigin(request)}/api/integrations/google/callback`

export function googleConnectUrl(redirectUri: string, state: string): string {
  const cfg = getAuthConfig().google
  const params = new URLSearchParams({
    client_id: cfg.clientId,
    redirect_uri: redirectUri,
    response_type: 'code',
    scope: DRIVE_SCOPES.join(' '),
    state,
    access_type: 'offline',
    // Force a refresh token even if the user has consented before.
    prompt: 'consent',
    include_granted_scopes: 'true',
  })
  if (cfg.hd) params.set('hd', cfg.hd)
  return `${AUTH_ENDPOINT}?${params.toString()}`
}

interface UserInfo {
  sub: string
  email?: string
}

/** Exchange the connect code, resolve the Google identity, and store the
 *  connection (encrypted) for the given Talaria user. */
export async function completeGoogleConnect(
  userId: string,
  code: string,
  redirectUri: string,
  nowMs: number,
): Promise<{ email: string | null }> {
  const cfg = getAuthConfig().google

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
    throw new Error(`google connect token exchange failed: ${tokenRes.status} ${await tokenRes.text()}`)
  }
  const tokens = (await tokenRes.json()) as {
    access_token?: string
    refresh_token?: string
    expires_in?: number
    scope?: string
  }
  if (!tokens.access_token) throw new Error('google connect: no access_token')

  const infoRes = await fetch(USERINFO_ENDPOINT, {
    headers: { Authorization: `Bearer ${tokens.access_token}` },
  })
  if (!infoRes.ok) throw new Error(`google userinfo failed: ${infoRes.status}`)
  const info = (await infoRes.json()) as UserInfo

  await saveConnection(userId, {
    googleSub: info.sub,
    email: info.email ?? null,
    scope: tokens.scope ?? DRIVE_SCOPES.join(' '),
    refreshToken: tokens.refresh_token ?? null,
    accessToken: tokens.access_token,
    expiresInSeconds: tokens.expires_in ?? null,
    nowMs,
  })
  return { email: info.email ?? null }
}
