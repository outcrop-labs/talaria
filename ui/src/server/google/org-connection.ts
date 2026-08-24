// The shared, org-wide Google connection (a single admin-configured account).
//
// General fleet agents — which have no human owner — act as THIS identity for
// Drive/Docs, so "the company" has one Google workspace the swarm can build in.
// Personal assistants instead act as their own owner (see agent-google.ts).
// Tokens are encrypted at rest, same as per-user connections.

import { db } from '../db/pg'
import { open, seal } from '../secretbox'
import { EXPIRY_SKEW_MS, REVOKE_ENDPOINT, requestRefresh, type GoogleConnectionStatus } from './connections'

interface OrgRow {
  google_sub: string
  email: string | null
  scope: string
  refresh_token_enc: string | null
  access_token_enc: string | null
  access_expires_at: string | null
  drive_folder_id: string | null
  calendar_id: string | null
  send_as: string | null
  shared_drive_id: string | null
  created_at: string
}

/** Where the org account's agents build. Empty strings normalize to null. */
export interface OrgTargets {
  /** Shared Drive or folder id for agent file creation (null → My Drive). */
  driveFolderId: string | null
  /** Calendar id for org events (null → 'primary'). */
  calendarId: string | null
  /** Verified send-as address for org mail (null → the account's own address). */
  sendAs: string | null
  /** The provisioned Shared Drive itself (null → not provisioned). */
  sharedDriveId: string | null
}

export async function getOrgConnectionStatus(): Promise<GoogleConnectionStatus & { targets: OrgTargets }> {
  const sql = await db()
  const [row] = await sql<OrgRow[]>`select * from google_org_connection where id = 1`
  const targets = {
    driveFolderId: row?.drive_folder_id ?? null,
    calendarId: row?.calendar_id ?? null,
    sendAs: row?.send_as ?? null,
    sharedDriveId: row?.shared_drive_id ?? null,
  }
  if (!row || !row.refresh_token_enc) return { connected: false, email: null, scope: [], connectedAt: null, targets }
  return {
    connected: true,
    email: row.email,
    scope: row.scope ? row.scope.split(' ').filter(Boolean) : [],
    connectedAt: row.created_at,
    targets,
  }
}

/** The org build targets alone (for execution paths). Null when not connected. */
export async function getOrgTargets(): Promise<OrgTargets> {
  const sql = await db()
  const [row] = await sql<OrgRow[]>`select drive_folder_id, calendar_id, send_as, shared_drive_id from google_org_connection where id = 1`
  return {
    driveFolderId: row?.drive_folder_id ?? null,
    calendarId: row?.calendar_id ?? null,
    sendAs: row?.send_as ?? null,
    sharedDriveId: row?.shared_drive_id ?? null,
  }
}

/** The connected org account's email alone, or null. The aliasing derivation
 *  reads this on the send path — plus-addresses hang off the org account, so
 *  no connection means no derived addresses (an agent's override still wins). */
export async function getOrgEmail(): Promise<string | null> {
  const sql = await db()
  const [row] = await sql<{ email: string | null; refresh: string | null }[]>`
    select email, refresh_token_enc as refresh from google_org_connection where id = 1
  `
  return row?.refresh ? row.email : null
}

/** Update the org build targets (admin). Only the fields PRESENT in the patch
 *  are written — a caller setting one target must not silently clear the other
 *  two (the admin form sends all three; provisioning sends one). Empty/blank
 *  values clear to null. The Shared Drive id has its own writer. */
export async function setOrgTargets(t: Partial<OrgTargets>): Promise<void> {
  const sql = await db()
  const norm = (v: string | null | undefined) => (v && v.trim() ? v.trim() : null)
  if (t.driveFolderId !== undefined) {
    await sql`update google_org_connection set drive_folder_id = ${norm(t.driveFolderId)}, updated_at = now() where id = 1`
  }
  if (t.calendarId !== undefined) {
    await sql`update google_org_connection set calendar_id = ${norm(t.calendarId)}, updated_at = now() where id = 1`
  }
  if (t.sendAs !== undefined) {
    await sql`update google_org_connection set send_as = ${norm(t.sendAs)}, updated_at = now() where id = 1`
  }
}

/** Record the provisioned Shared Drive (null clears). */
export async function setOrgSharedDrive(id: string | null): Promise<void> {
  const sql = await db()
  await sql`update google_org_connection set shared_drive_id = ${id}, updated_at = now() where id = 1`
}

export async function isOrgConnected(): Promise<boolean> {
  return (await getOrgConnectionStatus()).connected
}

interface SaveInput {
  googleSub: string
  email: string | null
  scope: string
  refreshToken?: string | null
  accessToken?: string | null
  expiresInSeconds?: number | null
  connectedBy: string | null
  nowMs: number
}

export async function saveOrgConnection(input: SaveInput): Promise<void> {
  const sql = await db()
  const refreshEnc = input.refreshToken ? seal(input.refreshToken) : null
  const accessEnc = input.accessToken ? seal(input.accessToken) : null
  const expiresAt =
    input.accessToken && input.expiresInSeconds ? new Date(input.nowMs + input.expiresInSeconds * 1000).toISOString() : null
  await sql`
    insert into google_org_connection
      (id, google_sub, email, scope, refresh_token_enc, access_token_enc, access_expires_at, connected_by, updated_at)
    values
      (1, ${input.googleSub}, ${input.email}, ${input.scope}, ${refreshEnc}, ${accessEnc}, ${expiresAt}, ${input.connectedBy}, now())
    on conflict (id) do update set
      google_sub = excluded.google_sub,
      email = excluded.email,
      scope = excluded.scope,
      refresh_token_enc = coalesce(excluded.refresh_token_enc, google_org_connection.refresh_token_enc),
      access_token_enc = excluded.access_token_enc,
      access_expires_at = excluded.access_expires_at,
      connected_by = excluded.connected_by,
      updated_at = now()
  `
}

export async function disconnectOrg(): Promise<void> {
  const sql = await db()
  const [row] = await sql<OrgRow[]>`select refresh_token_enc from google_org_connection where id = 1`
  if (row?.refresh_token_enc) {
    try {
      await fetch(REVOKE_ENDPOINT, {
        method: 'POST',
        headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ token: open(row.refresh_token_enc) }),
      })
    } catch {
      /* best-effort */
    }
  }
  await sql`delete from google_org_connection where id = 1`
}

/** A valid access token for the org connection. null ⇒ not connected. */
export async function getOrgAccessToken(nowMs: number): Promise<string | null> {
  const sql = await db()
  const [row] = await sql<OrgRow[]>`select * from google_org_connection where id = 1`
  if (!row || !row.refresh_token_enc) return null

  if (row.access_token_enc && row.access_expires_at) {
    const expiresMs = new Date(row.access_expires_at).getTime()
    if (expiresMs - EXPIRY_SKEW_MS > nowMs) {
      try {
        return open(row.access_token_enc)
      } catch {
        /* fall through to refresh */
      }
    }
  }

  let refreshed
  try {
    refreshed = await requestRefresh(open(row.refresh_token_enc))
  } catch (err) {
    if ((err as Error).name === 'InvalidGrant') {
      await sql`update google_org_connection set refresh_token_enc = null where id = 1`
    }
    throw err
  }
  const expiresAt = refreshed.expiresIn ? new Date(nowMs + refreshed.expiresIn * 1000).toISOString() : null
  await sql`
    update google_org_connection
    set access_token_enc = ${seal(refreshed.accessToken)}, access_expires_at = ${expiresAt}, updated_at = now()
    where id = 1
  `
  return refreshed.accessToken
}
