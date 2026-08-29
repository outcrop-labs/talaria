// The first-run claim — port of ui/src/server/auth/claim.ts. A fresh instance
// has no admin and no env bootstrap: the first person through /claim
// (password) or the first Google identity becomes the admin. Whoever deploys,
// owns.

use crate::users::Identity;
use sqlx::PgPool;

/// Distinct from MIGRATION_LOCK (8_314_207) in db/pg.ts. Transaction-scoped,
/// so it holds exactly as long as the claim's user-upsert + credential insert
/// take to commit — two simultaneous claims serialize, the loser sees the
/// re-check below fail, and the caller surfaces that as a 409 (password) or
/// falls through to the normal sign-in doors (Google).
pub const CLAIM_LOCK: i64 = 8_314_208;

/// The claimed admin row, in select order (id, sub, email, name, picture,
/// role) — the SessionUser constructor's input.
pub type ClaimedAdmin = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

/// True while NO user holds the admin role — the instance is still claimable.
/// A cheap read for surfacing the claim UI; the lock below is what actually
/// guards the write.
pub async fn instance_claimable(pg: &PgPool) -> Result<bool, sqlx::Error> {
    let (claimable,): (bool,) =
        sqlx::query_as("select not exists(select 1 from users where role = 'admin') as claimable")
            .fetch_one(pg)
            .await?;
    Ok(claimable)
}

/// Claim the instance: this identity becomes the admin. A password claim
/// stores its credential in the same transaction (an admin the password
/// cannot sign back in as is not a claim, it is a lockout). Returns None when
/// the race was lost — someone claimed between the caller's check and this
/// call. An identity that already exists as a member is promoted (they were
/// admitted once already; claiming is just the promotion).
pub async fn claim_admin(
    pg: &PgPool,
    identity: &Identity,
    password_hash: Option<&str>,
) -> Result<Option<ClaimedAdmin>, sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(CLAIM_LOCK)
        .execute(&mut *tx)
        .await?;
    let (open,): (bool,) =
        sqlx::query_as("select not exists(select 1 from users where role = 'admin') as claimable")
            .fetch_one(&mut *tx)
            .await?;
    if !open {
        // Dropping the transaction releases the lock and rolls back nothing
        // that mattered — the loser wrote nothing.
        return Ok(None);
    }

    let claimed: ClaimedAdmin = sqlx::query_as(
        "insert into users (sub, email, name, picture, role, last_seen_at) \
         values ($1, $2, $3, $4, 'admin', now()) \
         on conflict (sub) do update set \
           email = excluded.email, \
           name = case \
             when users.name is null or users.name = '' or users.name = users.email then excluded.name \
             else users.name \
           end, \
           picture = excluded.picture, \
           role = 'admin', \
           last_seen_at = now() \
         returning id::text, sub, email, name, picture, role",
    )
    .bind(&identity.sub)
    .bind(&identity.email)
    .bind(&identity.name)
    .bind(&identity.picture)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(hash) = password_hash {
        sqlx::query(
            "insert into user_password_credentials (user_id, email, password_hash) \
             values ($1::uuid, $2, $3) \
             on conflict (user_id) do update set \
               email = excluded.email, \
               password_hash = excluded.password_hash, \
               updated_at = now()",
        )
        .bind(&claimed.0)
        .bind(identity.email.as_deref().unwrap_or(&identity.sub))
        .bind(hash)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    // Org-wide boards are everyone's; a claim is a sign-in. Never fatal —
    // the admin still signs in; the next login retries.
    if let Err(e) = crate::users::join_org_wide_boards(pg, &claimed.0).await {
        tracing::error!(
            "[claim] could not join {} to org-wide boards: {e}",
            claimed.0
        );
    }
    Ok(Some(claimed))
}
