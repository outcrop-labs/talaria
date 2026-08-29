// Per-user Google connection — the status read the admin Google-client panel
// needs (ui/src/server/google/connections.ts getConnectionStatus). The token
// storage/vending side (refresh, revoke) ports with the integrations plane.

use crate::agent_auth::epoch_ms_to_iso;
use sqlx::PgPool;

/// The connection's public face (GoogleConnectionStatus) — wire order pinned.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    pub connected: bool,
    pub email: Option<String>,
    pub scope: Vec<String>,
    pub connected_at: Option<String>,
}

/// One row: connected means a refresh token is stored (TS checks truthiness,
/// so a blank cipher counts as not connected). Scope is the space-joined
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
