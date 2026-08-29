// DB-backed password accounts — the table that replaced env AUTH_USERS.
//
// One row per account in user_password_credentials: a scrypt hash
// (auth/password.ts), keyed by the unique lowercased email, linked to the
// users row it authenticates. Admission happened when an admin created the
// account (or at the first-run claim), so the login check consults ONLY the
// hash — there is no env list anymore and no plaintext that could ever verify.

import { db } from '../db/pg'
import { dummyHash, hashPassword, verifyPasswordHash } from './password'
import type { Identity, Role } from '../users'

export type LoginResult = Identity & { provider: 'password' }

export interface PasswordAccount {
  userId: string
  email: string
  name: string | null
  role: Role
  createdAt: string
  updatedAt: string
  lastUsedAt: string | null
}

/** Why a write refused — the route maps each to its own status/copy. */
export type WriteRefusal = 'email-taken' | 'no-email' | 'not-found'

/** Any password accounts at all? The login screen offers the password form
 *  only while at least one exists (and the login route refuses otherwise). */
export async function hasPasswordAccounts(): Promise<boolean> {
  const sql = await db()
  const rows = await sql`select exists(select 1 from user_password_credentials) as ok`
  return !!(rows[0] as { ok: boolean } | undefined)?.ok
}

/** Verify credentials against the table. An unknown email burns a dummy hash
 *  first, so a miss on the email fails exactly as slowly as a miss on the
 *  password — timing must not reveal which emails have accounts. Returns the
 *  identity, or null for either kind of miss. */
export async function verifyPasswordLogin(email: string, password: string): Promise<LoginResult | null> {
  const sql = await db()
  const rows = await sql`
    select c.user_id, c.email, c.password_hash, u.sub, u.name
    from user_password_credentials c
    join users u on u.id = c.user_id
    where c.email = ${email.trim().toLowerCase()}
  `
  const row = rows[0] as
    | { user_id: string; email: string; password_hash: string; sub: string; name: string | null }
    | undefined
  if (!row) {
    await verifyPasswordHash(password, await dummyHash())
    return null
  }
  if (!(await verifyPasswordHash(password, row.password_hash))) return null
  // Fire-and-forget: a failed activity stamp must not fail the sign-in.
  void sql`update user_password_credentials set last_used_at = now() where user_id = ${row.user_id}`.catch((e: unknown) =>
    console.error('[password-accounts] could not stamp last_used_at:', e),
  )
  return {
    sub: row.sub,
    email: row.email,
    name: row.name ?? row.email,
    picture: null,
    provider: 'password' as const,
  }
}

/** Create a password account. If a users row already carries this email —
 *  typically someone who signs in with Google — the credential attaches to
 *  THAT person rather than forking a second row for the same human
 *  (users.email is not unique; the credential's email is, and stays the login
 *  key even if the provider later rewrites the users row's email). */
export async function createPasswordAccount(input: {
  email: string
  password: string
  name?: string | null
}): Promise<{ ok: true; userId: string } | { ok: false; reason: WriteRefusal }> {
  const email = input.email.trim().toLowerCase()
  const sql = await db()
  return sql.begin(async (tx) => {
    const taken = await tx`select 1 from user_password_credentials where email = ${email}`
    if (taken.length > 0) return { ok: false, reason: 'email-taken' as const }

    // Lowest id wins should several rows share the email (it is not unique) —
    // a stable choice, and the credential's unique email keeps it moot.
    const existing = await tx`select id from users where lower(email) = ${email} order by id limit 1`
    let userId = (existing[0] as { id: string } | undefined)?.id
    if (!userId) {
      const sub = `password:${email}`
      const inserted = await tx`
        insert into users (sub, email, name, picture, role, last_seen_at)
        values (${sub}, ${email}, ${input.name?.trim() || email}, null, 'member', now())
        on conflict (sub) do update set email = excluded.email
        returning id
      `
      userId = (inserted[0] as { id: string }).id
    }
    await tx`
      insert into user_password_credentials (user_id, email, password_hash)
      values (${userId}, ${email}, ${await hashPassword(input.password)})
    `
    return { ok: true as const, userId }
  })
}

/** Set (or create) the password for an EXISTING user — the admin reset, and
 *  the admin's own "set my password". The account's email is the user's
 *  current email; a user with no email at all cannot have a password. */
export async function setPasswordAccountPassword(
  userId: string,
  password: string,
): Promise<{ ok: true; email: string } | { ok: false; reason: WriteRefusal }> {
  const sql = await db()
  const hash = await hashPassword(password)
  try {
    const rows = await sql`
      insert into user_password_credentials (user_id, email, password_hash)
      select u.id, lower(u.email), ${hash}
      from users u
      where u.id = ${userId} and u.email is not null and u.email <> ''
      on conflict (user_id) do update set password_hash = excluded.password_hash, updated_at = now()
      returning email
    `
    const email = (rows[0] as { email: string } | undefined)?.email
    if (!email) {
      // No such user, or one with no email for the account to hang from.
      const user = await sql`select 1 from users where id = ${userId}`
      return { ok: false, reason: user.length > 0 ? 'no-email' : 'not-found' }
    }
    return { ok: true as const, email }
  } catch (err) {
    // Someone else's account already holds this user's email as ITS login.
    if ((err as { code?: string }).code === '23505') return { ok: false, reason: 'email-taken' }
    throw err
  }
}

/** Remove the account — the PERSON stays; this only ends password sign-in.
 *  Returns the removed account's email (for the audit trail), or null when
 *  there was no account to remove. */
export async function removePasswordAccount(userId: string): Promise<string | null> {
  const sql = await db()
  const rows = await sql`delete from user_password_credentials where user_id = ${userId} returning email`
  return (rows[0] as { email: string } | undefined)?.email ?? null
}

/** The admin console's account list (Admin → People → Password accounts). */
export async function listPasswordAccounts(): Promise<PasswordAccount[]> {
  const sql = await db()
  const rows = await sql`
    select c.user_id as "userId", c.email, u.name, u.role,
           c.created_at as "createdAt", c.updated_at as "updatedAt", c.last_used_at as "lastUsedAt"
    from user_password_credentials c
    join users u on u.id = c.user_id
    order by c.email asc
  `
  return rows as unknown as PasswordAccount[]
}
