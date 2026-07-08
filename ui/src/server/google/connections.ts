// Per-user Google connection: token storage + access-token vending.
//
// Connecting stores an encrypted refresh_token (offline access). Callers ask for
// getAccessToken(userId) and get a fresh access token — we refresh transparently
// when the cached one is expired, re-encrypt, and hand back the bearer string.
// Nothing here ever returns a refresh token to callers.

import { getAuthConfig } from '../auth/config'
import { db } from '../db/pg'
import { open, seal } from '../secretbox'

const TOKEN_ENDPOINT = 'https://oauth2.googleapis.com/token'
const REVOKE_ENDPOINT = 'https://oauth2.googleapis.com/revoke'
// Refresh a little early so a token doesn't expire mid-request.
export const EXPIRY_SKEW_MS = 60_000

/** Exchange a refresh token for a fresh access token. Throws; a revoked token
 *  throws with name 'InvalidGrant' so callers can clear the dead connection.
 *  Shared by the per-user and org connections. */
export async function requestRefresh(refreshToken: string): Promise<{ accessToken: string; expiresIn: number | null }> {
  const cfg = getAuthConfig().google
  const res = await fetch(TOKEN_ENDPOINT, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({
      client_id: cfg.clientId,
      client_secret: cfg.clientSecret,
      refresh_token: refreshToken,
      grant_type: 'refresh_token',
    }),
  })
  if (!res.ok) {
    const body = await res.text()
    const err = new Error(`google token refresh failed: ${res.status} ${body}`)
    if (body.includes('invalid_grant')) err.name = 'InvalidGrant'
    throw err
  }
  const tokens = (await res.json()) as { access_token?: string; expires_in?: number }
  if (!tokens.access_token) throw new Error('google token refresh: no access_token')
  return { accessToken: tokens.access_token, expiresIn: tokens.expires_in ?? null }
}

export { REVOKE_ENDPOINT }

export interface GoogleConnectionStatus {
  connected: boolean
  email: string | null
  scope: string[]
  connectedAt: string | null
}

interface ConnectionRow {
  user_id: string
  google_sub: string
  email: string | null
  scope: string
  refresh_token_enc: string | null
  access_token_enc: string | null
  access_expires_at: string | null
  created_at: string
}

/** Public status for the UI — never exposes tokens. */
export async function getConnectionStatus(userId: string): Promise<GoogleConnectionStatus> {
  const sql = await db()
  const [row] = await sql<ConnectionRow[]>`
    select * from google_connections where user_id = ${userId}
  `
  if (!row || !row.refresh_token_enc) {
    return { connected: false, email: null, scope: [], connectedAt: null }
  }
  return {
    connected: true,
    email: row.email,
    scope: row.scope ? row.scope.split(' ').filter(Boolean) : [],
    connectedAt: row.created_at,
  }
}

/** True if the user has a usable (refresh-token-bearing) connection. */
export async function isConnected(userId: string): Promise<boolean> {
  return (await getConnectionStatus(userId)).connected
}

interface SaveInput {
  googleSub: string
  email: string | null
  scope: string
  refreshToken?: string | null
  accessToken?: string | null
  expiresInSeconds?: number | null
  nowMs: number
}

/** Upsert a connection after an OAuth exchange. A missing refresh_token (Google
 *  omits it when the user re-consents without prompt=consent) preserves the one
 *  already stored, so a re-connect never silently drops offline access. */
export async function saveConnection(userId: string, input: SaveInput): Promise<void> {
  const sql = await db()
  const refreshEnc = input.refreshToken ? seal(input.refreshToken) : null
  const accessEnc = input.accessToken ? seal(input.accessToken) : null
  const expiresAt =
    input.accessToken && input.expiresInSeconds
      ? new Date(input.nowMs + input.expiresInSeconds * 1000).toISOString()
      : null
  await sql`
    insert into google_connections
      (user_id, google_sub, email, scope, refresh_token_enc, access_token_enc, access_expires_at, updated_at)
    values
      (${userId}, ${input.googleSub}, ${input.email}, ${input.scope}, ${refreshEnc}, ${accessEnc}, ${expiresAt}, now())
    on conflict (user_id) do update set
      google_sub = excluded.google_sub,
      email = excluded.email,
      scope = excluded.scope,
      -- keep the existing refresh token if this exchange didn't return one
      refresh_token_enc = coalesce(excluded.refresh_token_enc, google_connections.refresh_token_enc),
      access_token_enc = excluded.access_token_enc,
      access_expires_at = excluded.access_expires_at,
      updated_at = now()
  `
}

/** Forget a user's connection and best-effort revoke the token at Google. */
export async function disconnect(userId: string): Promise<void> {
  const sql = await db()
  const [row] = await sql<ConnectionRow[]>`
    select refresh_token_enc from google_connections where user_id = ${userId}
  `
  if (row?.refresh_token_enc) {
    try {
      const token = open(row.refresh_token_enc)
      await fetch(REVOKE_ENDPOINT, {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ token }),
      })
    } catch {
      // Revocation is best-effort; we still drop the row.
    }
  }
  await sql`delete from google_connections where user_id = ${userId}`
}

/** A valid access token for the user, refreshing if needed. null ⇒ not connected. */
export async function getAccessToken(userId: string, nowMs: number): Promise<string | null> {
  const sql = await db()
  const [row] = await sql<ConnectionRow[]>`
    select * from google_connections where user_id = ${userId}
  `
  if (!row || !row.refresh_token_enc) return null

  // Reuse the cached access token while it's comfortably valid.
  if (row.access_token_enc && row.access_expires_at) {
    const expiresMs = new Date(row.access_expires_at).getTime()
    if (expiresMs - EXPIRY_SKEW_MS > nowMs) {
      try {
        return open(row.access_token_enc)
      } catch {
        // fall through to refresh
      }
    }
  }

  const refreshToken = open(row.refresh_token_enc)
  let refreshed
  try {
    refreshed = await requestRefresh(refreshToken)
  } catch (err) {
    // A revoked/expired refresh token means the connection is dead — clear it so
    // the UI prompts a reconnect instead of erroring forever.
    if ((err as Error).name === 'InvalidGrant') {
      await sql`update google_connections set refresh_token_enc = null where user_id = ${userId}`
    }
    throw err
  }
  const expiresAt = refreshed.expiresIn ? new Date(nowMs + refreshed.expiresIn * 1000).toISOString() : null
  await sql`
    update google_connections
    set access_token_enc = ${seal(refreshed.accessToken)}, access_expires_at = ${expiresAt}, updated_at = now()
    where user_id = ${userId}
  `
  return refreshed.accessToken
}
