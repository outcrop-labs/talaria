// DB-backed password accounts — the LOGIN side of
// ui/src/server/auth/password-accounts.ts. One row per account in
// user_password_credentials: a scrypt hash (password.rs), keyed by the unique
// lowercased email. Admission happened when an admin created the account (or
// at the first-run claim), so the login check consults ONLY the hash. The
// admin write paths (create/set/remove/list — Admin → People) port with the
// admin surfaces in batch 3.

use crate::password::{dummy_hash, verify_password_hash};
use sqlx::PgPool;

/// The verified identity a login resolves to (LoginResult — provider fixed).
pub struct LoginIdentity {
    pub sub: String,
    pub email: String,
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// Any password accounts at all? The login screen offers the password form
/// only while at least one exists (and the login route refuses otherwise).
pub async fn has_password_accounts(pg: &PgPool) -> Result<bool, sqlx::Error> {
    let (ok,): (bool,) =
        sqlx::query_as("select exists(select 1 from user_password_credentials) as ok")
            .fetch_one(pg)
            .await?;
    Ok(ok)
}

/// The dummy burn for an unknown email — same cost as a real verify, on the
/// blocking pool like one.
async fn burn_dummy(password: &str) {
    let p = password.to_string();
    let _ = tokio::task::spawn_blocking(move || verify_password_hash(&p, dummy_hash())).await;
}

/// Verify credentials against the table. An unknown email burns the dummy hash
/// first, so a miss on the email fails exactly as slowly as a miss on the
/// password — timing must not reveal which emails have accounts. Returns the
/// identity, or None for either kind of miss.
pub async fn verify_password_login(
    pg: &PgPool,
    email: &str,
    password: &str,
) -> Result<Option<LoginIdentity>, sqlx::Error> {
    let row: Option<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "select c.user_id::text, c.email, c.password_hash, u.sub, u.name \
         from user_password_credentials c join users u on u.id = c.user_id \
         where c.email = $1",
    )
    .bind(email.trim().to_lowercase())
    .fetch_optional(pg)
    .await?;
    let Some((user_id, email, hash, sub, name)) = row else {
        burn_dummy(password).await;
        return Ok(None);
    };
    // scrypt is ~100ms of CPU — the blocking pool, never an async worker.
    let (p, h) = (password.to_string(), hash);
    if !tokio::task::spawn_blocking(move || verify_password_hash(&p, &h))
        .await
        .unwrap_or(false)
    {
        return Ok(None);
    }
    // Fire-and-forget: a failed activity stamp must not fail the sign-in.
    let pool = pg.clone();
    let stamp_user = user_id.clone();
    tokio::spawn(async move {
        if let Err(e) = sqlx::query(
            "update user_password_credentials set last_used_at = now() where user_id::text = $1",
        )
        .bind(&stamp_user)
        .execute(&pool)
        .await
        {
            tracing::error!("[password-accounts] could not stamp last_used_at: {e}");
        }
    });
    Ok(Some(LoginIdentity {
        sub,
        name: name.or(Some(email.clone())), // row.name ?? row.email
        email,
        picture: None,
    }))
}
