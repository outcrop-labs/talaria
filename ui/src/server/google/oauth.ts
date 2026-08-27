// Google OAuth — the ONE leaf both flows build on.
//
// Two flows share this machinery and differ only in policy:
//   CONNECT (this file)   durable, offline access to Drive/Docs/Calendar/Mail
//                         (access_type=offline, prompt=consent → a refresh
//                         token we store encrypted), per-user or shared-org.
//   LOGIN (server/auth/google.ts)  proves who you are (access_type=online, no
//                         stored token). That file keeps the login POLICY —
//                         the domain gate, the doors, identity shaping — and
//                         exchanges its code through `exchangeGoogleTokens`
//                         here. Separate redirect URIs so the flows never
//                         cross-wire; one exchange, one userinfo shape, one
//                         set of endpoints.
//
// Until this consolidation, the endpoints, the token-exchange fetch, and the
// userinfo fetch lived twice (once per flow) under two UserInfo types for one
// endpoint, and the two connect callbacks carried the same body verbatim.

import { getAuthConfig } from '../auth/config'
import { clearStateCookie, getSessionUser, parseCookies, stateMatches, STATE_COOKIE } from '../auth/session'
import { resolveGoogleClient, type GoogleClient } from './client-config'
import { saveConnection } from './connections'
import { saveOrgConnection } from './org-connection'

// Exported for the login flow's consent URL; the token/userinfo endpoints are
// leaf-internal (every caller goes through exchangeGoogleTokens).
export const AUTH_ENDPOINT = 'https://accounts.google.com/o/oauth2/v2/auth'
const TOKEN_ENDPOINT = 'https://oauth2.googleapis.com/token'
const USERINFO_ENDPOINT = 'https://openidconnect.googleapis.com/v1/userinfo'

/** External origin for redirect URIs: AUTH_PUBLIC_URL, else derived from the
 *  request. Lives in this leaf (not auth/google.ts, where it was born) so the
 *  login module can import the exchange without closing a cycle. */
export function resolveOrigin(request: Request): string {
  const configured = getAuthConfig().publicUrl
  if (configured) return configured
  const url = new URL(request.url)
  const proto = request.headers.get('x-forwarded-proto') ?? url.protocol.replace(':', '')
  const host = request.headers.get('x-forwarded-host') ?? request.headers.get('host') ?? url.host
  return `${proto}://${host}`
}

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

/** The ONE userinfo shape — one endpoint, one type. The login flow reads
 *  name/picture/hd from it; the connect flows read sub/email. The old two
 *  spellings (`UserInfo`, `GoogleUserInfo`) described the same response. */
export interface GoogleUserInfo {
  sub: string
  email?: string
  email_verified?: boolean
  name?: string
  picture?: string
  hd?: string
}

interface ExchangedTokens {
  access_token?: string
  refresh_token?: string
  expires_in?: number
  scope?: string
}

/** Exchange an auth code for tokens + the Google identity — the fetch BOTH
 *  flows run (login shapes the identity, connect stores the tokens). Throws
 *  when no client is configured; callers gate on the enabled checks first. */
export async function exchangeGoogleTokens(code: string, redirectUri: string): Promise<{ tokens: ExchangedTokens; info: GoogleUserInfo }> {
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
  const tokens = (await tokenRes.json()) as ExchangedTokens
  if (!tokens.access_token) throw new Error('google token exchange: no access_token')

  const infoRes = await fetch(USERINFO_ENDPOINT, { headers: { Authorization: `Bearer ${tokens.access_token}` } })
  if (!infoRes.ok) throw new Error(`google userinfo failed: ${infoRes.status}`)
  return { tokens, info: (await infoRes.json()) as GoogleUserInfo }
}

/** Exchange the connect code and store the connection (encrypted) for a user. */
export async function completeGoogleConnect(
  userId: string,
  code: string,
  redirectUri: string,
  nowMs: number,
): Promise<{ email: string | null }> {
  const { tokens, info } = await exchangeGoogleTokens(code, redirectUri)
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
  const { tokens, info } = await exchangeGoogleTokens(code, redirectUri)
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

/** The connect flows' shared callback body: gate on the integration, require
 *  a session (the org flavor adds admin), verify the state cookie, exchange +
 *  store, and bounce back with a status flag. The two routes differ only in
 *  what a stored connection MEANS (whose Drive/calendar/mail) and where the
 *  human lands — everything else was verbatim. */
export async function handleConnectCallback(
  request: Request,
  opts: {
    complete: (userId: string, code: string, redirectUri: string, nowMs: number) => Promise<unknown>
    redirectUri: string
    /** Status → landing path (e.g. `/settings/connections?google=`). */
    landing: (status: string) => string
    logTag: string
    adminOnly?: boolean
  },
): Promise<Response> {
  const back = (status: string): Response =>
    new Response(null, {
      status: 302,
      headers: { Location: opts.landing(status), 'Set-Cookie': clearStateCookie() },
    })

  if (!(await googleIntegrationEnabled())) return back('disabled')
  const user = await getSessionUser(request)
  if (!user) return new Response(null, { status: 302, headers: { Location: '/login' } })
  if (opts.adminOnly && user.role !== 'admin') return back('forbidden')

  const url = new URL(request.url)
  const code = url.searchParams.get('code')
  const state = url.searchParams.get('state')
  const cookieState = parseCookies(request)[STATE_COOKIE]

  if (url.searchParams.get('error')) return back('denied')
  if (!code || !state || !cookieState || !stateMatches(state, cookieState)) return back('bad_state')

  try {
    await opts.complete(user.id, code, opts.redirectUri, Date.now())
  } catch (err) {
    if (import.meta.env.DEV) console.error(`[${opts.logTag}] connect failed:`, err)
    return back('exchange_failed')
  }
  return back('connected')
}
