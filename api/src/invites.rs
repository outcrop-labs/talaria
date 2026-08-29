// Invites — the third door in (after env allow-lists and verified sign-up
// domains), port of the admission reads of ui/src/server/invites.ts. An admin
// invites an email; the invitee gets a /join link and signs in with Google on
// that address. The create/revoke/list console (and its email) ports with the
// admin surfaces in batch 3.

use sqlx::PgPool;

/// Admission door: a live invite exists for this (provider-verified) email —
/// unaccepted, unrevoked, unexpired.
pub async fn invite_allowed(pg: &PgPool, email: Option<&str>) -> Result<bool, sqlx::Error> {
    let Some(email) = email else { return Ok(false) };
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from invites \
         where email = $1 and accepted_at is null and revoked_at is null and expires_at > now()",
    )
    .bind(email.to_lowercase())
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

/// Stamp acceptance once the invitee's account exists.
pub async fn mark_invite_accepted(
    pg: &PgPool,
    email: &str,
    user_id: &str,
) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "update invites set accepted_at = now(), accepted_user_id = $2::uuid \
         where email = $1 and accepted_at is null and revoked_at is null",
    )
    .bind(email.to_lowercase())
    .bind(user_id)
    .execute(pg)
    .await
    .map(|r| r.rows_affected())
}
