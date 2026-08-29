// The first-run claim. A fresh instance has no admin and no env bootstrap —
// the first person through /claim (password) or the first Google identity
// (when Google login is on) becomes the admin. Whoever deploys, owns.

import { db } from '../db/pg'
import type { Identity, User } from '../users'

// Distinct from MIGRATION_LOCK (8_314_207) in db/pg.ts. Transaction-scoped,
// so it holds exactly as long as the claim's user-upsert + credential insert
// take to commit — two simultaneous claims serialize, the loser sees the
// re-check below fail, and the caller surfaces that as a 409 (password) or
// falls through to the normal sign-in doors (Google).
export const CLAIM_LOCK = 8_314_208

/** True while NO user holds the admin role — the instance is still claimable.
 *  A cheap read for surfacing the claim UI; the lock below is what actually
 *  guards the write. */
export async function instanceClaimable(): Promise<boolean> {
  const sql = await db()
  const rows = await sql`select not exists(select 1 from users where role = 'admin') as claimable`
  return !!(rows[0] as { claimable: boolean } | undefined)?.claimable
}

/** Claim the instance: this identity becomes the admin. A password claim
 *  stores its credential in the same transaction (an admin the password
 *  cannot sign back in as is not a claim, it is a lockout). Returns null when
 *  the race was lost — someone claimed between the caller's check and this
 *  call. An identity that already exists as a member is promoted (they were
 *  admitted once already; claiming is just the promotion). */
export async function claimAdmin(identity: Identity, passwordHash?: string): Promise<User | null> {
  const sql = await db()
  const user = await sql.begin(async (tx) => {
    await tx`select pg_advisory_xact_lock(${CLAIM_LOCK})`
    const open = await tx`select not exists(select 1 from users where role = 'admin') as claimable`
    if (!(open[0] as { claimable: boolean }).claimable) return null

    const rows = await tx`
      insert into users (sub, email, name, picture, role, last_seen_at)
      values (${identity.sub}, ${identity.email}, ${identity.name}, ${identity.picture}, 'admin', now())
      on conflict (sub) do update set
        email = excluded.email,
        -- same display-name rule as upsertUser: a name the user chose survives,
        -- provider defaults (empty, or name = email) get filled in.
        name = case
          when users.name is null or users.name = '' or users.name = users.email then excluded.name
          else users.name
        end,
        picture = excluded.picture,
        role = 'admin',
        last_seen_at = now()
      returning id, sub, email, name, picture, role
    `
    const claimed = rows[0] as User
    if (passwordHash) {
      await tx`
        insert into user_password_credentials (user_id, email, password_hash)
        values (${claimed.id}, ${identity.email ?? identity.sub}, ${passwordHash})
        on conflict (user_id) do update set
          email = excluded.email,
          password_hash = excluded.password_hash,
          updated_at = now()
      `
    }
    return claimed
  })
  if (user) {
    // Org-wide boards are everyone's; a claim is a sign-in. Dynamic import for
    // the same cycle-avoidance reason as upsertUser (boards imports users).
    // Never fatal — the admin still signs in; the next login retries.
    const { joinOrgWideBoards } = await import('../boards')
    await joinOrgWideBoards(user.id).catch((e: unknown) =>
      console.error(`[claim] could not join ${user.id} to org-wide boards:`, e),
    )
  }
  return user
}
