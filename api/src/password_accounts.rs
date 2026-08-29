// DB-backed password accounts — port of
// ui/src/server/auth/password-accounts.ts: the login side (verify) and the
// admin console's write side (create/set/remove/list — Admin → People). One
// row per account in user_password_credentials: a scrypt hash (password.rs),
// keyed by the unique lowercased email. Admission happened when an admin
// created the account (or at the first-run claim), so the login check
// consults ONLY the hash.

use crate::agent_auth::epoch_ms_to_iso;
use crate::password::{dummy_hash, hash_password, verify_password_hash};
use sqlx::{PgPool, Row};

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

// ── The admin console's write side (Admin → People) ──────────────────────────

/// Why a write refused — the route maps each to its own status/copy.
#[derive(Debug, PartialEq)]
pub enum WriteRefusal {
    EmailTaken,
    NoEmail,
    NotFound,
}

/// The admin list entry (PasswordAccount) — wire order pinned. Timestamps are
/// epoch-ms → ISO, the shape every JS Date takes on the wire.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordAccount {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub role: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_used_at: Option<String>,
}

/// Create a password account. If a users row already carries this email —
/// typically someone who signs in with Google — the credential attaches to
/// THAT person rather than forking a second row for the same human (users
/// .email is not unique; the credential's email is, and stays the login key).
/// `name` already passed the route's optional-max check.
pub async fn create_password_account(
    pg: &PgPool,
    email: &str,
    password: &str,
    name: Option<&str>,
) -> Result<Result<String, WriteRefusal>, sqlx::Error> {
    let email = email.trim().to_lowercase();
    let mut tx = pg.begin().await?;

    let taken: Option<(i32,)> =
        sqlx::query_as("select 1 from user_password_credentials where email = $1")
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await?;
    if taken.is_some() {
        return Ok(Err(WriteRefusal::EmailTaken));
    }

    // Lowest id wins should several rows share the email (it is not unique) —
    // a stable choice, and the credential's unique email keeps it moot.
    let existing: Option<(String,)> =
        sqlx::query_as("select id::text from users where lower(email) = $1 order by id limit 1")
            .bind(&email)
            .fetch_optional(&mut *tx)
            .await?;
    let user_id = match existing {
        Some((id,)) => id,
        None => {
            // `input.name?.trim() || email` — a blank name falls back to the
            // email as the display name.
            let display = name
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or(&email);
            let sub = format!("password:{email}");
            let (id,): (String,) = sqlx::query_as(
                "insert into users (sub, email, name, picture, role, last_seen_at) \
                 values ($1, $2, $3, null, 'member', now()) \
                 on conflict (sub) do update set email = excluded.email \
                 returning id::text",
            )
            .bind(&sub)
            .bind(&email)
            .bind(display)
            .fetch_one(&mut *tx)
            .await?;
            id
        }
    };

    // scrypt is ~100ms of CPU — the blocking pool, like every hash here.
    let pw = password.to_string();
    let hash = match tokio::task::spawn_blocking(move || hash_password(&pw)).await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("[password-accounts] hash task panicked: {e}");
            return Err(sqlx::Error::WorkerCrashed);
        }
    };
    sqlx::query(
        "insert into user_password_credentials (user_id, email, password_hash) \
         values ($1::uuid, $2, $3)",
    )
    .bind(&user_id)
    .bind(&email)
    .bind(&hash)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(Ok(user_id))
}

/// Set (or create) the password for an EXISTING user — the admin reset, and
/// the admin's own "set my password". The account's email is the user's
/// current email; a user with no email at all cannot have a password.
pub async fn set_password_account_password(
    pg: &PgPool,
    user_id: &str,
    password: &str,
) -> Result<Result<String, WriteRefusal>, sqlx::Error> {
    let pw = password.to_string();
    let hash = match tokio::task::spawn_blocking(move || hash_password(&pw)).await {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("[password-accounts] hash task panicked: {e}");
            return Err(sqlx::Error::WorkerCrashed);
        }
    };
    let insert = sqlx::query(
        "insert into user_password_credentials (user_id, email, password_hash) \
         select u.id, lower(u.email), $2 \
         from users u \
         where u.id = $1::uuid and u.email is not null and u.email <> '' \
         on conflict (user_id) do update set password_hash = excluded.password_hash, updated_at = now() \
         returning email",
    )
    .bind(user_id)
    .bind(&hash)
    .fetch_optional(pg)
    .await;
    match insert {
        Ok(Some(row)) => Ok(Ok(row.get::<String, _>("email"))),
        Ok(None) => {
            // No such user, or one with no email for the account to hang from.
            let probe: Option<(i32,)> = sqlx::query_as("select 1 from users where id = $1::uuid")
                .bind(user_id)
                .fetch_optional(pg)
                .await?;
            Ok(Err(if probe.is_some() {
                WriteRefusal::NoEmail
            } else {
                WriteRefusal::NotFound
            }))
        }
        Err(e) => {
            // Someone else's account already holds this user's email as ITS
            // login — the unique-email collision, TS's 23505.
            if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23505") {
                return Ok(Err(WriteRefusal::EmailTaken));
            }
            Err(e)
        }
    }
}

/// Remove the account — the PERSON stays; this only ends password sign-in.
/// Returns the removed account's email (for the audit trail), or None when
/// there was no account to remove.
pub async fn remove_password_account(
    pg: &PgPool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "delete from user_password_credentials where user_id = $1::uuid returning email",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(email,)| email))
}

/// (userId, email, name, role, created/updated/lastUsed epoch ms) — one row.
type AccountRow = (
    String,
    String,
    Option<String>,
    String,
    i64,
    i64,
    Option<i64>,
);

/// The admin console's account list (Admin → People → Password accounts).
pub async fn list_password_accounts(pg: &PgPool) -> Result<Vec<PasswordAccount>, sqlx::Error> {
    let rows: Vec<AccountRow> = sqlx::query_as(
        "select c.user_id::text, c.email, u.name, u.role, \
                    (trunc(extract(epoch from c.created_at) * 1000))::bigint, \
                    (trunc(extract(epoch from c.updated_at) * 1000))::bigint, \
                    (trunc(extract(epoch from c.last_used_at) * 1000))::bigint \
             from user_password_credentials c join users u on u.id = c.user_id \
             order by c.email asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(user_id, email, name, role, created_ms, updated_ms, last_used_ms)| PasswordAccount {
                user_id,
                email,
                name,
                role,
                created_at: epoch_ms_to_iso(created_ms),
                updated_at: epoch_ms_to_iso(updated_ms),
                last_used_at: last_used_ms.map(epoch_ms_to_iso),
            },
        )
        .collect())
}
