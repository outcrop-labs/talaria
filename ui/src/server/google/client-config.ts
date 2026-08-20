// The Google OAuth CLIENT credentials — the one piece of the Google Workspace
// integration that used to require editing ui/.env. Admins register the client
// in the Admin UI (Admin → Org → Google Workspace); the record lives in
// app_settings with the secret SEALED (secretbox), the same contract as email
// and GitHub.
//
// ENV REMAINS THE FALLBACK: a deployment with AUTH_GOOGLE_CLIENT_ID/_SECRET in
// the env keeps working untouched, and a half-saved Admin record (id without a
// secret) never takes over from working env credentials. The record wins only
// when it is complete, so exactly one client is ever active.
//
// THE ACTIVE CLIENT SERVES EVERY GOOGLE FLOW — workspace connect (per-user and
// org), token refresh, and Google LOGIN when AUTH_GOOGLE_ENABLED is also set.
// One OAuth client in Google Cloud Console, one set of credentials here.
import { getSetting, setSetting } from '../audit'
import { db } from '../db/pg'
import { open, seal } from '../secretbox'

const KEY = 'google_oauth_client'

interface StoredClient {
  clientId: string
  clientSecretEnc: string | null
  hd: string | null
}

export interface GoogleClient {
  clientId: string
  clientSecret: string
  hd: string | null
  /** Where the active credentials came from — the admin panel labels it. */
  source: 'admin' | 'env'
}

/** The one client every Google flow uses, or null when nothing usable is
 *  configured anywhere. DB record wins when complete; env is the fallback. */
export async function resolveGoogleClient(): Promise<GoogleClient | null> {
  const stored = await getSetting<Partial<StoredClient>>(KEY, {})
  if (stored.clientId && stored.clientSecretEnc) {
    try {
      return { clientId: stored.clientId, clientSecret: open(stored.clientSecretEnc), hd: stored.hd ?? null, source: 'admin' }
    } catch {
      /* sealed with a lost/rotated key — fall through to env rather than die */
    }
  }
  const env = process.env
  if (env.AUTH_GOOGLE_CLIENT_ID && env.AUTH_GOOGLE_CLIENT_SECRET) {
    return {
      clientId: env.AUTH_GOOGLE_CLIENT_ID,
      clientSecret: env.AUTH_GOOGLE_CLIENT_SECRET,
      hd: env.AUTH_GOOGLE_HD?.trim() || null,
      source: 'env',
    }
  }
  return null
}

export interface GoogleClientStatus {
  /** A complete client exists (Admin record or env) — the integration runs. */
  configured: boolean
  source: 'admin' | 'env' | null
  clientId: string
  secretSet: boolean
  hd: string | null
}

/** Status for the admin panel — says which source is live and surfaces a
 *  half-saved record (id stored, secret missing) so the panel can finish it
 *  instead of silently falling back to env. */
export async function googleClientStatus(): Promise<GoogleClientStatus> {
  const client = await resolveGoogleClient()
  if (client) return { configured: true, source: client.source, clientId: client.clientId, secretSet: true, hd: client.hd }
  const stored = await getSetting<Partial<StoredClient>>(KEY, {})
  return { configured: false, source: null, clientId: stored.clientId ?? '', secretSet: !!stored.clientSecretEnc, hd: stored.hd ?? null }
}

/** Patch the stored client. `clientSecret` follows the house rule: a non-empty
 *  string seals + stores it, null/'' clears it, undefined leaves it untouched. */
export async function setGoogleClientConfig(patch: {
  clientId?: string
  clientSecret?: string | null
  hd?: string | null
}): Promise<void> {
  const cur = await getSetting<Partial<StoredClient>>(KEY, {})
  const secret = patch.clientSecret?.trim() ?? ''
  const hd = patch.hd?.trim() ?? ''
  const next: StoredClient = {
    clientId: patch.clientId !== undefined ? patch.clientId.trim() : cur.clientId ?? '',
    clientSecretEnc:
      patch.clientSecret !== undefined ? (secret ? seal(secret) : null) : cur.clientSecretEnc ?? null,
    hd: patch.hd !== undefined ? (hd || null) : cur.hd ?? null,
  }
  if (!next.clientId) throw new Error('a client id is required')
  await setSetting(KEY, next)
}

/** Drop the stored record. Env credentials (if any) take over again — clearing
 *  the Admin record never disables a working env deployment. */
export async function clearGoogleClientConfig(): Promise<void> {
  const sql = await db()
  await sql`delete from app_settings where key = ${KEY}`
}

/** Whether GOOGLE LOGIN is offered. Login is a policy decision, not a
 *  credential decision: an admin registering a workspace client must not
 *  silently open a new way into the instance, so the env flag stays the gate —
 *  but it now works with credentials from either source. */
export async function googleLoginEnabled(): Promise<boolean> {
  const flag = process.env.AUTH_GOOGLE_ENABLED === '1' || process.env.AUTH_GOOGLE_ENABLED === 'true'
  return flag && !!(await resolveGoogleClient())
}
