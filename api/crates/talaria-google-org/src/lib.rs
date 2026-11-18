// The shared, org-wide Google connection (google_org_connection id=1) — the
// token half that the agent Drive export path needs. The admin save /
// disconnect / status writers stay with the integrations plane: general fleet
// agents — which have no human owner — act as THIS identity for Drive/Docs,
// so "the company" has one Google workspace the swarm can build in. Personal
// assistants instead act as their own owner (google/agent.rs). Tokens are
// encrypted at rest, same as per-user connections.

use sqlx::PgPool;
use talaria_agent_auth::epoch_ms_to_iso;
use talaria_google_connections::{GrantRow, TokenError, vend_grant};
use talaria_secretbox::SecretBox;

/// Where the org account's agents build. Empty strings are never stored —
/// the writers normalize blanks to null.
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
/// unconfigured zero value.
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

/// A valid access token for the org connection, or None when not connected —
/// the same read-through as the per-user one (`vend_grant` on the org row):
/// reuse the cached token while comfortably valid, else refresh with the
/// client that minted the grant and re-seal; a revoked grant clears the
/// connection so the admin UI prompts a reconnect instead of erroring
/// forever, and a refused client flags it and keeps it — the login domain
/// gate reads this row's refresh token, so a bad client secret must never
/// open it.
pub async fn get_org_access_token(
    pg: &PgPool,
    sb: &SecretBox,
    now_ms: i64,
) -> Result<Option<String>, TokenError> {
    Ok(vend_grant(pg, sb, &GrantRow::Org, now_ms)
        .await?
        .map(|v| v.token))
}

// ── The admin/status half ────────────────────────────────────────────────────

use serde::Serialize;

/// The org connection's public face: the status shape plus the build targets.
/// Wire order pinned — `{available, connected, email, scope, connectedAt,
/// targets}` is what the admin panel reads.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetsWire {
    pub drive_folder_id: Option<String>,
    pub calendar_id: Option<String>,
    pub send_as: Option<String>,
    pub shared_drive_id: Option<String>,
}

impl From<&OrgTargets> for TargetsWire {
    fn from(t: &OrgTargets) -> Self {
        TargetsWire {
            drive_folder_id: t.drive_folder_id.clone(),
            calendar_id: t.calendar_id.clone(),
            send_as: t.send_as.clone(),
            shared_drive_id: t.shared_drive_id.clone(),
        }
    }
}

/// The per-user status shape plus `targets` — the admin panel's org read.
/// `client_id` and `flagged` as on `ConnectionStatus`: the client that
/// minted the stored grant, and Google's words from its last refusal.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgStatus {
    pub connected: bool,
    pub email: Option<String>,
    pub scope: Vec<String>,
    pub connected_at: Option<String>,
    pub targets: TargetsWire,
    pub client_id: Option<String>,
    pub flagged: Option<String>,
}

/// One row of the connection, read whole (email, scope, refresh cipher,
/// the four targets, created-at epoch ms, minting client, flag).
#[allow(clippy::type_complexity)]
type OrgStatusRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
    Option<String>,
);

async fn org_status_row(pg: &PgPool) -> Result<Option<OrgStatusRow>, sqlx::Error> {
    sqlx::query_as(
        "select email, scope, refresh_token_enc, drive_folder_id, calendar_id, send_as, \
                shared_drive_id, (trunc(extract(epoch from created_at) * 1000))::bigint, \
                oauth_client_id, reconnect_reason \
         from google_org_connection where id = 1",
    )
    .fetch_optional(pg)
    .await
}

/// The shared org connection's full status. No row, or a row without a
/// refresh token, is the unconfigured zero shape (truthiness: a blank cipher
/// counts as not connected).
pub async fn get_org_connection_status(pg: &PgPool) -> Result<OrgStatus, sqlx::Error> {
    let row = org_status_row(pg).await?;
    let (
        email,
        scope,
        refresh,
        drive_folder_id,
        calendar_id,
        send_as,
        shared_drive_id,
        created_ms,
        client_id,
        flagged,
    ) = row.unwrap_or((None, None, None, None, None, None, None, 0, None, None));
    let targets = TargetsWire {
        drive_folder_id,
        calendar_id,
        send_as,
        shared_drive_id,
    };
    let connected = refresh.is_some_and(|s| !s.is_empty());
    if !connected {
        return Ok(OrgStatus {
            connected: false,
            email: None,
            scope: vec![],
            connected_at: None,
            targets,
            client_id: None,
            flagged: None,
        });
    }
    Ok(OrgStatus {
        connected: true,
        client_id,
        flagged,
        email,
        scope: scope
            .map(|s| {
                s.split(' ')
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        connected_at: (created_ms != 0).then(|| epoch_ms_to_iso(created_ms)),
        targets,
    })
}

/// A partial targets patch: `None` = the field was ABSENT from the body (not
/// written); `Some(None)` = written null (cleared). Only the fields PRESENT
/// in the patch are written — a caller setting one target must not silently
/// clear the other three (the admin form sends all three; provisioning sends
/// one). The Shared Drive id has its own writer.
#[derive(Default)]
pub struct OrgTargetsPatch {
    pub drive_folder_id: Option<Option<String>>,
    pub calendar_id: Option<Option<String>>,
    pub send_as: Option<Option<String>>,
}

/// Update the org build targets (admin). Empty/blank values clear to null.
pub async fn set_org_targets(pg: &PgPool, patch: &OrgTargetsPatch) -> Result<(), sqlx::Error> {
    let norm = |v: Option<String>| {
        v.and_then(|v| {
            let t = v.trim();
            (!t.is_empty()).then(|| t.to_string())
        })
    };
    if let Some(drive_folder_id) = patch.drive_folder_id.clone() {
        sqlx::query(
            "update google_org_connection set drive_folder_id = $1, updated_at = now() where id = 1",
        )
        .bind(norm(drive_folder_id))
        .execute(pg)
        .await?;
    }
    if let Some(calendar_id) = patch.calendar_id.clone() {
        sqlx::query(
            "update google_org_connection set calendar_id = $1, updated_at = now() where id = 1",
        )
        .bind(norm(calendar_id))
        .execute(pg)
        .await?;
    }
    if let Some(send_as) = patch.send_as.clone() {
        sqlx::query(
            "update google_org_connection set send_as = $1, updated_at = now() where id = 1",
        )
        .bind(norm(send_as))
        .execute(pg)
        .await?;
    }
    Ok(())
}

/// Record the provisioned Shared Drive (null clears).
pub async fn set_org_shared_drive(pg: &PgPool, id: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update google_org_connection set shared_drive_id = $1, updated_at = now() where id = 1",
    )
    .bind(id)
    .execute(pg)
    .await?;
    Ok(())
}

/// What an org OAuth exchange hands the saver.
pub struct SaveOrgConnection<'a> {
    pub google_sub: &'a str,
    pub email: Option<&'a str>,
    pub scope: &'a str,
    pub refresh_token: Option<&'a str>,
    pub access_token: Option<&'a str>,
    pub expires_in_seconds: Option<i64>,
    pub connected_by: Option<&'a str>,
    /// The OAuth client the exchange ran on (`SaveConnection`'s).
    pub oauth_client_id: &'a str,
    pub now_ms: i64,
}

/// Store the SHARED org connection (id=1 upsert; a missing refresh token
/// preserves the stored one, and the client that minted it, same as the
/// per-user save).
pub async fn save_org_connection(
    pg: &PgPool,
    sb: &SecretBox,
    input: &SaveOrgConnection<'_>,
) -> Result<(), String> {
    let seal_opt = |v: Option<&str>| -> Result<Option<String>, String> {
        match v {
            Some(v) => Ok(Some(
                sb.seal(v)
                    .map_err(|e| format!("google org token seal: {e}"))?,
            )),
            None => Ok(None),
        }
    };
    let refresh_enc = seal_opt(input.refresh_token)?;
    let access_enc = seal_opt(input.access_token)?;
    let expires_at = match (input.access_token, input.expires_in_seconds) {
        (Some(t), Some(secs)) if !t.is_empty() && secs != 0 => {
            Some(epoch_ms_to_iso(input.now_ms + secs * 1000))
        }
        _ => None,
    };
    sqlx::query(
        "insert into google_org_connection \
             (id, google_sub, email, scope, refresh_token_enc, access_token_enc, access_expires_at, connected_by, oauth_client_id, updated_at) \
         values (1, $1, $2, $3, $4, $5, $6::timestamptz, $7::uuid, $8, now()) \
         on conflict (id) do update set \
             google_sub = excluded.google_sub, \
             email = excluded.email, \
             scope = excluded.scope, \
             refresh_token_enc = coalesce(excluded.refresh_token_enc, google_org_connection.refresh_token_enc), \
             oauth_client_id = case when excluded.refresh_token_enc is null \
                 then google_org_connection.oauth_client_id else excluded.oauth_client_id end, \
             reconnect_reason = case when excluded.refresh_token_enc is null \
                 then google_org_connection.reconnect_reason end, \
             reconnect_flagged_at = case when excluded.refresh_token_enc is null \
                 then google_org_connection.reconnect_flagged_at end, \
             last_mcp_refusal = case when excluded.refresh_token_enc is null \
                 then google_org_connection.last_mcp_refusal end, \
             access_token_enc = excluded.access_token_enc, \
             access_expires_at = excluded.access_expires_at, \
             connected_by = excluded.connected_by, \
             updated_at = now()",
    )
    .bind(input.google_sub)
    .bind(input.email)
    .bind(input.scope)
    .bind(refresh_enc)
    .bind(access_enc)
    .bind(expires_at)
    .bind(input.connected_by)
    .bind(input.oauth_client_id)
    .execute(pg)
    .await
    .map_err(|e| format!("google org connection save: {e}"))?;
    Ok(())
}

/// Forget the org connection, and revoke its grant at Google unless another
/// stored connection holds the same grant — an admin who connected the org
/// with their own account keeps their personal connection.
pub async fn disconnect_org(
    pg: &PgPool,
    sb: &SecretBox,
) -> talaria_google_connections::RevokeOutcome {
    use talaria_google_connections::{GrantRow, forget_grant_best_effort};
    forget_grant_best_effort(pg, sb, &GrantRow::Org).await
}

/// The connected org account's email alone, or null. The aliasing derivation
/// reads this on the send path — plus-addresses hang off the org account, so
/// no connection means no derived addresses (an agent's override still wins).
pub async fn get_org_email(pg: &PgPool) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("select email, refresh_token_enc from google_org_connection where id = 1")
            .fetch_optional(pg)
            .await?;
    Ok(row
        .filter(|(_, refresh)| refresh.as_deref().is_some_and(|s| !s.is_empty()))
        .and_then(|(email, _)| email))
}
