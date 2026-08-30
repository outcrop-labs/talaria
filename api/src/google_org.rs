// The shared, org-wide Google connection (google_org_connection id=1) — the
// token half that the agent Drive export path needs (ui/src/server/google/
// org-connection.ts getOrgTargets/getOrgAccessToken). The admin save /
// disconnect / status writers stay with the integrations plane: general fleet
// agents — which have no human owner — act as THIS identity for Drive/Docs,
// so "the company" has one Google workspace the swarm can build in. Personal
// assistants instead act as their own owner (google_agent.rs). Tokens are
// encrypted at rest, same as per-user connections.

use crate::agent_auth::epoch_ms_to_iso;
use crate::google_connections::{EXPIRY_SKEW_MS, TokenError, request_refresh};
use crate::secretbox::SecretBox;
use sqlx::PgPool;

/// Where the org account's agents build (org-connection.ts OrgTargets).
/// Empty strings are never stored — the writers normalize blanks to null.
#[derive(Debug, Clone, Default)]
pub struct OrgTargets {
    /// Shared Drive or folder id for agent file creation (null → My Drive).
    pub drive_folder_id: Option<String>,
    /// Calendar id for org events (null → 'primary').
    pub calendar_id: Option<String>,
    /// Verified send-as address for org mail (null → the account's own).
    pub send_as: Option<String>,
    /// The provisioned Shared Drive itself (null → not provisioned).
    pub shared_drive_id: Option<String>,
}

/// The org build targets alone (for execution paths). No row at all is the
/// unconfigured zero value, same as TS.
pub async fn get_org_targets(pg: &PgPool) -> Result<OrgTargets, sqlx::Error> {
    // (drive_folder_id, calendar_id, send_as, shared_drive_id) — one row.
    type TargetRow = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<TargetRow> = sqlx::query_as(
        "select drive_folder_id, calendar_id, send_as, shared_drive_id \
         from google_org_connection where id = 1",
    )
    .fetch_optional(pg)
    .await?;
    Ok(match row {
        Some((drive_folder_id, calendar_id, send_as, shared_drive_id)) => OrgTargets {
            drive_folder_id,
            calendar_id,
            send_as,
            shared_drive_id,
        },
        None => OrgTargets::default(),
    })
}

/// A valid access token for the org connection, or None when not connected
/// (org-connection.ts getOrgAccessToken — same read-through shape as the
/// per-user get_access_token: reuse the cached token while comfortably valid,
/// else refresh and re-seal; a revoked refresh token clears the connection so
/// the admin UI prompts a reconnect instead of erroring forever).
pub async fn get_org_access_token(
    pg: &PgPool,
    sb: &SecretBox,
    now_ms: i64,
) -> Result<Option<String>, TokenError> {
    let row: Option<(Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
        "select refresh_token_enc, access_token_enc, \
                (trunc(extract(epoch from access_expires_at) * 1000))::bigint \
         from google_org_connection where id = 1",
    )
    .fetch_optional(pg)
    .await
    .map_err(|e| TokenError::Other(format!("google org connection read: {e}")))?;
    let Some((refresh_enc, access_enc, access_expires_ms)) = row else {
        return Ok(None);
    };
    let refresh_enc = refresh_enc.filter(|s| !s.is_empty());
    let Some(refresh_enc) = refresh_enc else {
        return Ok(None);
    };

    if let (Some(enc), Some(expires_ms)) = (access_enc, access_expires_ms)
        && expires_ms - EXPIRY_SKEW_MS > now_ms
        && let Ok(token) = sb.open(&enc)
    {
        return Ok(Some(token));
    }

    let refresh_token = sb
        .open(&refresh_enc)
        .map_err(|e| TokenError::Other(format!("google org refresh token unseal: {e}")))?;
    let (access_token, expires_in) = match request_refresh(pg, sb, &refresh_token).await {
        Ok(ok) => ok,
        Err(e @ TokenError::InvalidGrant(_)) => {
            let _ = sqlx::query(
                "update google_org_connection set refresh_token_enc = null where id = 1",
            )
            .execute(pg)
            .await;
            return Err(e);
        }
        Err(e) => return Err(e),
    };
    let expires_at = expires_in.map(|secs| epoch_ms_to_iso(now_ms + secs * 1000));
    let sealed = sb
        .seal(&access_token)
        .map_err(|e| TokenError::Other(format!("google org access token seal: {e}")))?;
    let _ = sqlx::query(
        "update google_org_connection \
         set access_token_enc = $2, access_expires_at = $3, updated_at = now() \
         where id = 1",
    )
    .bind(sealed)
    .bind(expires_at)
    .execute(pg)
    .await;
    Ok(Some(access_token))
}
