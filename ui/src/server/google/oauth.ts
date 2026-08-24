// Google connect flow — DISTINCT from google login (server/auth/google.ts).
//
// Login proves who you are (access_type=online, no stored token). Connecting
// grants Talaria durable, offline access to your Drive/Docs (access_type=offline,
// prompt=consent → a refresh token we store encrypted). Separate redirect URI so
// the two flows never cross-wire.

import { resolveOrigin } from '../auth/google'
import { resolveGoogleClient, type GoogleClient } from './client-config'
import { saveConnection } from './connections'
import { saveOrgConnection } from './org-connection'

const AUTH_ENDPOINT = 'https://accounts.google.com/o/oauth2/v2/auth'
const TOKEN_ENDPOINT = 'https://oauth2.googleapis.com/token'
const USERINFO_ENDPOINT = 'https://openidconnect.googleapis.com/v1/userinfo'

// Least-privilege for export/import: drive.file grants access only to files the
// app itself creates or the user explicitly opens with it — not the whole Drive.
export const WORKSPACE_SCOPES = [
  'openid',
  'email',
  // Create + manage files the app itself makes (export).
  'https://www.googleapis.com/auth/drive.file',
  // Read metadata + content of the user's Drive files (browse + import).
  'https://www.googleapis.com/auth/drive.readonly',
  // View + edit calendar events (agenda + create).
  'https://www.googleapis.com/auth/calendar.events',
  // Read + ORGANIZE mail: labels, mark-read, archive (gmail.modify — it covers
  // reads too, and the organize tools batch-apply labels). Swapped in for
  // gmail.readonly when organizing shipped; a connection granted before then
  // needs one reconnect to pick the scope up (the routes say so when they hit it).
  'https://www.googleapis.com/auth/gmail.modify',
  'https://www.googleapis.com/auth/gmail.send',
]

// The ORG connect asks for more than a personal one: provisioning the shared
// workspace means CREATING its containers — an org calendar plus its share
// rules, a whole Shared Drive plus its domain permission — and the per-file /
// per-event scopes above cannot create either. Full `calendar` covers
// calendars.insert + acl.insert (and everything calendar.events did); full
// `drive` covers drives.insert + permissions (and everything drive.file and
// drive.readonly did). An org connected before this array existed keeps its
// old scopes until one reconnect — the provisioning route says so when it
// checks (same convention the gmail.modify swap set).
export const ORG_CONNECT_SCOPES = [
  'openid',
  'email',
  // Create + share the org calendar; edit its (and the primary's) events.
  'https://www.googleapis.com/auth/calendar',
  // Create the Shared Drive and grant the domain access to it.
  'https://www.googleapis.com/auth/drive',
  'https://www.googleapis.com/auth/gmail.modify',
  'https://www.googleapis.com/auth/gmail.send',
]

/** Whether the Google integration can run at all — an OAuth client exists,
 *  from the Admin UI record or the env fallback (same client as login). */
export async function googleIntegrationEnabled(): Promise<boolean> {
  return !!(await resolveGoogleClient())
}

export const googleConnectRedirectUri = (request: Request) =>
  `${resolveOrigin(request)}/api/integrations/google/callback`

export const googleOrgConnectRedirectUri = (request: Request) =>
  `${resolveOrigin(request)}/api/integrations/google/org/callback`

/** The consent URL for a connect flow. `scopes` defaults to the personal
 *  set; the org connect passes ORG_CONNECT_SCOPES (see its comment). */
export async function googleConnectUrl(redirectUri: string, state: string, scopes: string[] = WORKSPACE_SCOPES): Promise<string> {
  const cfg = (await resolveGoogleClient())!
  const params = new URLSearchParams({
    client_id: cfg.clientId,
    redirect_uri: redirectUri,
    response_type: 'code',
    scope: scopes.join(' '),
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

interface ExchangedTokens {
  access_token?: string
  refresh_token?: string
  expires_in?: number
  scope?: string
}

/** Exchange an auth code for tokens + the Google identity. Shared by the
 *  per-user and org connect flows. Throws when no client is configured —
 *  callers gate on `googleIntegrationEnabled` first. */
async function exchangeConnectCode(code: string, redirectUri: string): Promise<{ tokens: ExchangedTokens; info: UserInfo }> {
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
    throw new Error(`google connect token exchange failed: ${tokenRes.status} ${await tokenRes.text()}`)
  }
  const tokens = (await tokenRes.json()) as ExchangedTokens
  if (!tokens.access_token) throw new Error('google connect: no access_token')

  const infoRes = await fetch(USERINFO_ENDPOINT, { headers: { Authorization: `Bearer ${tokens.access_token}` } })
  if (!infoRes.ok) throw new Error(`google userinfo failed: ${infoRes.status}`)
  return { tokens, info: (await infoRes.json()) as UserInfo }
}

/** Exchange the connect code and store the connection (encrypted) for a user. */
export async function completeGoogleConnect(
  userId: string,
  code: string,
  redirectUri: string,
  nowMs: number,
): Promise<{ email: string | null }> {
  const { tokens, info } = await exchangeConnectCode(code, redirectUri)
  await saveConnection(userId, {
    googleSub: info.sub,
    email: info.email ?? null,
    scope: tokens.scope ?? WORKSPACE_SCOPES.join(' '),
    refreshToken: tokens.refresh_token ?? null,
    accessToken: tokens.access_token,
    expiresInSeconds: tokens.expires_in ?? null,
    nowMs,
  })
  return { email: info.email ?? null }
}

/** Exchange the connect code and store it as the SHARED org connection. */
export async function completeGoogleOrgConnect(
  connectedBy: string | null,
  code: string,
  redirectUri: string,
  nowMs: number,
): Promise<{ email: string | null }> {
  const { tokens, info } = await exchangeConnectCode(code, redirectUri)
  await saveOrgConnection({
    googleSub: info.sub,
    email: info.email ?? null,
    scope: tokens.scope ?? ORG_CONNECT_SCOPES.join(' '),
    refreshToken: tokens.refresh_token ?? null,
    accessToken: tokens.access_token,
    expiresInSeconds: tokens.expires_in ?? null,
    connectedBy,
    nowMs,
  })
  return { email: info.email ?? null }
}
