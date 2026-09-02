// Per-user Google connection — the status read the admin Google-client panel
// needs, plus the token vending every Google surface reads through. The OAuth
// exchange itself lives in oauth.rs.

use crate::agent_auth::epoch_ms_to_iso;
use crate::gateway::provider::http;
use crate::google::client::resolve_google_client;
use crate::secretbox::SecretBox;
use axum::http::header;
use sqlx::PgPool;

/// Refresh a little early so a token doesn't expire mid-request.
pub const EXPIRY_SKEW_MS: i64 = 60_000;
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// The connection's public face — wire order pinned.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub connected: bool,
    pub email: Option<String>,
    pub scope: Vec<String>,
    pub connected_at: Option<String>,
}

/// One row: connected means a refresh token is stored (truthiness — a blank
/// cipher counts as not connected). Scope is the space-joined
/// grant list, null-or-blank → empty.
/// (email, scope, refresh cipher, created-at epoch ms) — one row.
type ConnRow = (Option<String>, Option<String>, Option<String>, Option<i64>);

pub async fn get_connection_status(
    pg: &PgPool,
    user_id: &str,
) -> Result<ConnectionStatus, sqlx::Error> {
    let row: Option<ConnRow> = sqlx::query_as(
        "select email, scope, refresh_token_enc, \
                    (trunc(extract(epoch from created_at) * 1000))::bigint \
             from google_connections where user_id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    let Some((email, scope, refresh_enc, created_ms)) = row else {
        return Ok(ConnectionStatus {
            connected: false,
            email: None,
            scope: vec![],
            connected_at: None,
        });
    };
    let connected = refresh_enc.as_deref().is_some_and(|s| !s.is_empty());
    if !connected {
        return Ok(ConnectionStatus {
            connected: false,
            email: None,
            scope: vec![],
            connected_at: None,
        });
    }
    let scope = scope
        .as_deref()
        .map(|s| {
            s.split(' ')
                .filter(|p| !p.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    Ok(ConnectionStatus {
        connected: true,
        email,
        scope,
        connected_at: created_ms.map(epoch_ms_to_iso),
    })
}

/// Why a token vending failed. `InvalidGrant` is its own case because it means
/// the CONNECTION IS DEAD (revoked or expired at Google's end) — the caller
/// clears the stored refresh token so the UI prompts a reconnect instead of
/// erroring forever, rather than treating it like a transient failure.
#[derive(Debug)]
pub enum TokenError {
    InvalidGrant(String),
    Other(String),
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenError::InvalidGrant(m) | TokenError::Other(m) => write!(f, "{m}"),
        }
    }
}

/// Exchange a refresh token for a fresh access token. Shared by the per-user
/// and org connections. Errors carry the status and body verbatim — the only
/// consumer is the log.
pub async fn request_refresh(
    pg: &PgPool,
    sb: &SecretBox,
    refresh_token: &str,
) -> Result<(String, Option<i64>), TokenError> {
    let Some(cfg) = resolve_google_client(pg, sb).await else {
        return Err(TokenError::Other("no google client configured".into()));
    };
    // Built — and dropped — before the await, on purpose: the Serializer holds
    // a fn-pointer encoder that is not Send, and a binding still in scope at
    // the await would make every caller's future (the brief's, namely)
    // non-Send. The block is what ends its scope.
    let body = {
        let mut form = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in [
            ("client_id", cfg.client_id.as_str()),
            ("client_secret", cfg.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ] {
            form.append_pair(k, v);
        }
        form.finish()
    };
    let res = http()
        .post(TOKEN_ENDPOINT)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| TokenError::Other(format!("google token refresh request: {e}")))?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        let err = format!("google token refresh failed: {status} {body}");
        return Err(if body.contains("invalid_grant") {
            TokenError::InvalidGrant(err)
        } else {
            TokenError::Other(err)
        });
    }
    let tokens: serde_json::Value = res
        .json()
        .await
        .map_err(|e| TokenError::Other(format!("google token refresh body: {e}")))?;
    let access_token = tokens
        .get("access_token")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| TokenError::Other("google token refresh: no access_token".into()))?
        .to_string();
    Ok((
        access_token,
        tokens.get("expires_in").and_then(|v| v.as_i64()),
    ))
}

/// A valid access token for the user, refreshing if needed. `Ok(None)` ⇒ not
/// connected. The cached token is reused while it is comfortably valid; an
/// unopenable cipher falls through to a refresh rather than erroring the
/// caller.
///
/// Read-through shape: (refresh cipher, access cipher, access expiry epoch ms).
/// The expiry is read as epoch ms in SQL — parsing PG's `timestamptz::text`
/// spelling back into a clock would just be another way to be wrong.
type TokenRow = (Option<String>, Option<String>, Option<i64>);

pub async fn get_access_token(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
) -> Result<Option<String>, TokenError> {
    let row: Option<TokenRow> = sqlx::query_as(
        "select refresh_token_enc, access_token_enc, \
                (trunc(extract(epoch from access_expires_at) * 1000))::bigint \
         from google_connections where user_id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| TokenError::Other(format!("google connection read: {e}")))?;
    let Some((refresh_enc, access_enc, access_expires_ms)) = row else {
        return Ok(None);
    };
    let refresh_enc = refresh_enc.filter(|s| !s.is_empty());
    let Some(refresh_enc) = refresh_enc else {
        return Ok(None);
    };

    // Reuse the cached access token while it's comfortably valid. An expired
    // or unopenable cache simply fails the chain and falls through to refresh.
    if let (Some(enc), Some(expires_ms)) = (access_enc, access_expires_ms)
        && expires_ms - EXPIRY_SKEW_MS > now_ms
        && let Ok(token) = sb.open(&enc)
    {
        return Ok(Some(token));
    }

    let refresh_token = sb
        .open(&refresh_enc)
        .map_err(|e| TokenError::Other(format!("google refresh token unseal: {e}")))?;
    let (access_token, expires_in) = match request_refresh(pg, sb, &refresh_token).await {
        Ok(ok) => ok,
        Err(e @ TokenError::InvalidGrant(_)) => {
            // A revoked/expired refresh token means the connection is dead —
            // clear it so the UI prompts a reconnect instead of erroring forever.
            let _ = sqlx::query(
                "update google_connections set refresh_token_enc = null where user_id = $1::uuid",
            )
            .bind(user_id)
            .execute(pg)
            .await;
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    let expires_at = expires_in.map(|secs| epoch_ms_to_iso(now_ms + secs * 1000));
    let sealed = sb
        .seal(&access_token)
        .map_err(|e| TokenError::Other(format!("google access token seal: {e}")))?;
    let _ = sqlx::query(
        "update google_connections set access_token_enc = $2, access_expires_at = $3::timestamptz, updated_at = now() \
         where user_id = $1::uuid",
    )
    .bind(user_id)
    .bind(sealed)
    .bind(expires_at)
    .execute(pg)
    .await;
    Ok(Some(access_token))
}

// ── The connect flow's writers ───────────────────────────────────────────────

/// What an OAuth exchange hands the saver. A missing refresh token (Google
/// omits it when the user re-consents without prompt=consent) preserves the
/// one already stored, so a re-connect never silently drops offline access.
pub struct SaveConnection<'a> {
    pub google_sub: &'a str,
    pub email: Option<&'a str>,
    pub scope: &'a str,
    pub refresh_token: Option<&'a str>,
    pub access_token: Option<&'a str>,
    pub expires_in_seconds: Option<i64>,
    pub now_ms: i64,
}

/// Upsert a connection after an OAuth exchange.
pub async fn save_connection(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    input: &SaveConnection<'_>,
) -> Result<(), String> {
    let seal_opt = |v: Option<&str>| -> Result<Option<String>, String> {
        match v {
            Some(v) => Ok(Some(
                sb.seal(v).map_err(|e| format!("google token seal: {e}"))?,
            )),
            None => Ok(None),
        }
    };
    let refresh_enc = seal_opt(input.refresh_token)?;
    let access_enc = seal_opt(input.access_token)?;
    let expires_at = match (input.access_token, input.expires_in_seconds) {
        // Truthiness: an empty-string token or a zero/null expiry writes
        // no expiry at all.
        (Some(t), Some(secs)) if !t.is_empty() && secs != 0 => Some(
            crate::agent_auth::epoch_ms_to_iso(input.now_ms + secs * 1000),
        ),
        _ => None,
    };
    sqlx::query(
        "insert into google_connections \
             (user_id, google_sub, email, scope, refresh_token_enc, access_token_enc, access_expires_at, updated_at) \
         values ($1::uuid, $2, $3, $4, $5, $6, $7::timestamptz, now()) \
         on conflict (user_id) do update set \
             google_sub = excluded.google_sub, \
             email = excluded.email, \
             scope = excluded.scope, \
             refresh_token_enc = coalesce(excluded.refresh_token_enc, google_connections.refresh_token_enc), \
             access_token_enc = excluded.access_token_enc, \
             access_expires_at = excluded.access_expires_at, \
             updated_at = now()",
    )
    .bind(user_id)
    .bind(input.google_sub)
    .bind(input.email)
    .bind(input.scope)
    .bind(refresh_enc)
    .bind(access_enc)
    .bind(expires_at)
    .execute(pg)
    .await
    .map_err(|e| format!("google connection save: {e}"))?;
    Ok(())
}

/// Forget a user's connection and best-effort revoke the token at Google.
/// Revocation failing (network, already-revoked) still drops the row — the
/// stored state and Google's may disagree for the token's remaining lifetime,
/// which is the safer direction of the two to disagree in.
pub async fn disconnect(pg: &PgPool, sb: &SecretBox, user_id: &str) {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select refresh_token_enc from google_connections where user_id = $1::uuid")
            .bind(user_id)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();
    if let Some((Some(enc),)) = row
        && let Ok(token) = sb.open(&enc)
    {
        // Built — and dropped — before the await: the Serializer's encoder
        // fn-pointer is not Send, and a binding still in scope at the await
        // would make this future non-Send.
        let body = {
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            form.append_pair("token", &token);
            form.finish()
        };
        let _ = http()
            .post("https://oauth2.googleapis.com/revoke")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await;
    }
    let _ = sqlx::query("delete from google_connections where user_id = $1::uuid")
        .bind(user_id)
        .execute(pg)
        .await;
}

/// require_token's failure: `NotConnected` means get_access_token answered
/// null — a state, not an error; `Failed` is everything the refresh path can
/// throw.
#[derive(Debug)]
pub enum RequireError {
    NotConnected,
    Failed(String),
}

impl From<TokenError> for RequireError {
    fn from(e: TokenError) -> Self {
        RequireError::Failed(e.to_string())
    }
}

/// A valid access token, or NotConnected for an unconnected user. Drive,
/// Gmail, and Calendar all read through this one door — the error KIND the
/// routes branch on comes from here, once.
pub async fn require_token(
    pg: &PgPool,
    sb: &SecretBox,
    user_id: &str,
    now_ms: i64,
) -> Result<String, RequireError> {
    match get_access_token(pg, sb, user_id, now_ms).await? {
        Some(token) => Ok(token),
        None => Err(RequireError::NotConnected),
    }
}
