// Invites — the third door in (after env allow-lists and verified sign-up
// domains). An admin invites an email; the invitee gets a transactional
// email with a /join link; signing in with Google on that address admits
// them and marks the invite accepted. Invites expire, revoke instantly, and
// re-send with a fresh token.
import { randomBytes } from 'node:crypto'
import { db } from './db/pg'
import { emailShell, sendEmail } from './email'
import { orgProfile } from './org'
import { instanceBaseUrl } from './instance'
import { escapeHtml } from './html'

export interface Invite {
  id: string
  email: string
  invitedBy: string | null
  createdAt: string
  expiresAt: string
  acceptedAt: string | null
  revokedAt: string | null
}

const ROW = `id, email, invited_by as "invitedBy", created_at as "createdAt",
  expires_at as "expiresAt", accepted_at as "acceptedAt", revoked_at as "revokedAt"`
const TTL_DAYS = 14

export async function listInvites(): Promise<Invite[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${ROW} from invites order by created_at desc limit 200`)) as unknown as Invite[]
}

async function sendInviteEmail(email: string, token: string, invitedBy: string, origin: string | null): Promise<{ ok: boolean; error?: string }> {
  const org = await orgProfile()
  const base = (await instanceBaseUrl()) ?? origin
  const orgName = org.name?.trim() || 'the team'
  const link = base ? `${base}/join?token=${token}` : null
  // Every interpolation is operator- or user-sourced and the invite email
  // regex permits quotes and angle brackets — escape all of it. The text
  // alternative below needs no escaping.
  const esc = escapeHtml
  const r = await sendEmail({
    to: email,
    subject: `${invitedBy} invited you to join ${orgName} on Talaria`,
    html: emailShell(
      `Join ${esc(orgName)}`,
      `<p><strong>${esc(invitedBy)}</strong> invited you to ${esc(orgName)}'s Talaria workspace — where the team and its AI agents work together.</p>` +
        (link
          ? `<p style="margin:24px 0"><a href="${esc(link)}" style="background:#1a1a18;color:#ffffff;text-decoration:none;padding:10px 18px;border-radius:8px;font-size:14px">Accept the invite</a></p>` +
            `<p style="font-size:12px;color:#8a8a84">Or open ${esc(link)} — then sign in with Google using this address (${esc(email)}).</p>`
          : `<p>Sign in with Google using this address (${esc(email)}) at your team's Talaria instance to join.</p>`),
      `Sent by Talaria for ${esc(orgName)}`,
    ),
    text: `${invitedBy} invited you to join ${orgName} on Talaria.${link ? ` Accept: ${link}` : ''} Sign in with Google using ${email}.`,
  })
  return r.ok ? { ok: true } : { ok: false, error: r.error }
}

/** Create (or refresh) an invite and send the email. An existing pending
 *  invite for the address is re-issued with a fresh token + expiry. */
export async function createInvite(
  email: string,
  invitedBy: string,
  origin: string | null,
): Promise<{ invite: Invite; emailSent: boolean; emailError?: string }> {
  const e = email.trim().toLowerCase()
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(e)) throw new Error('that does not look like an email address')
  const sql = await db()
  const token = randomBytes(24).toString('base64url')
  const rows = (await sql`
    insert into invites (email, token, invited_by, expires_at)
    values (${e}, ${token}, ${invitedBy}, now() + make_interval(days => ${TTL_DAYS}))
    returning ${sql.unsafe(ROW)}
  `) as unknown as Invite[]
  // One live invite per address: retire older pending ones.
  await sql`
    update invites set revoked_at = now()
    where email = ${e} and id <> ${rows[0]!.id} and accepted_at is null and revoked_at is null
  `
  const sent = await sendInviteEmail(e, token, invitedBy, origin)
  return { invite: rows[0]!, emailSent: sent.ok, emailError: sent.error }
}

export async function revokeInvite(id: string): Promise<void> {
  const sql = await db()
  await sql`update invites set revoked_at = now() where id = ${id} and accepted_at is null`
}

/** Public join-page lookup: what does this token invite, and is it live? */
export async function inviteByToken(token: string): Promise<{ email: string; invitedBy: string | null; orgName: string } | null> {
  const sql = await db()
  const rows = (await sql`
    select email, invited_by as "invitedBy" from invites
    where token = ${token} and accepted_at is null and revoked_at is null and expires_at > now()
  `) as unknown as Array<{ email: string; invitedBy: string | null }>
  if (!rows[0]) return null
  const org = await orgProfile()
  return { ...rows[0], orgName: org.name?.trim() || 'the team' }
}

/** Admission door: a live invite exists for this (provider-verified) email. */
export async function inviteAllowed(email: string | null | undefined): Promise<boolean> {
  if (!email) return false
  const sql = await db()
  const rows = await sql`
    select 1 as ok from invites
    where email = ${email.toLowerCase()} and accepted_at is null and revoked_at is null and expires_at > now()
  `
  return rows.length > 0
}

/** Stamp acceptance once the invitee's account exists. */
export async function markInviteAccepted(email: string, userId: string): Promise<void> {
  const sql = await db()
  await sql`
    update invites set accepted_at = now(), accepted_user_id = ${userId}
    where email = ${email.toLowerCase()} and accepted_at is null and revoked_at is null
  `
}
