// The Google OAuth CLIENT credentials — port of
// ui/src/server/google/client-config.ts. The record lives in app_settings
// (`google_oauth_client`) with the secret SEALED (secretbox); env
// (AUTH_GOOGLE_CLIENT_ID/_SECRET) remains the fallback, and the record wins
// only when it is complete — a half-saved record never takes over from
// working env credentials, so exactly one client is ever active.

use crate::gateway::settings::{get_setting, set_setting};
use crate::secretbox::SecretBox;
use serde_json::Value;
use sqlx::PgPool;

const KEY: &str = "google_oauth_client";
const LOGIN_KEY: &str = "google_login_enabled";

pub struct GoogleClient {
    pub client_id: String,
    pub client_secret: String,
    pub hd: Option<String>,
    /// Where the active credentials came from — the admin panel labels it.
    pub source: &'static str,
}

/// The one client every Google flow uses, or None when nothing usable is
/// configured anywhere. DB record wins when complete; env is the fallback.
pub async fn resolve_google_client(pg: &PgPool, sb: &SecretBox) -> Option<GoogleClient> {
    let stored = get_setting(pg, KEY, serde_json::Value::Object(Default::default())).await;
    let client_id = stored
        .get("clientId")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    let enc = stored
        .get("clientSecretEnc")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    if let (Some(client_id), Some(enc)) = (client_id, enc) {
        // Sealed with a lost/rotated key → fall through to env rather than die.
        if let Ok(client_secret) = sb.open(enc) {
            // `stored.hd ?? null`: absent → null; a present string rides as-is.
            let hd = stored.get("hd").and_then(|v| v.as_str()).map(String::from);
            return Some(GoogleClient {
                client_id: client_id.to_string(),
                client_secret,
                hd,
                source: "admin",
            });
        }
    }
    let env = std::env::var;
    let id = env("AUTH_GOOGLE_CLIENT_ID").unwrap_or_default();
    let secret = env("AUTH_GOOGLE_CLIENT_SECRET").unwrap_or_default();
    if !id.is_empty() && !secret.is_empty() {
        return Some(GoogleClient {
            client_id: id,
            client_secret: secret,
            hd: Some(env("AUTH_GOOGLE_HD").unwrap_or_default().trim().to_string())
                .filter(|s| !s.is_empty()),
            source: "env",
        });
    }
    None
}

/// Status for the admin panel — says which source is live and surfaces a
/// half-saved record (id stored, secret missing) so the panel can finish it
/// instead of silently falling back to env. Wire order pinned by the SPA.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleClientStatus {
    pub configured: bool,
    pub source: Option<&'static str>,
    pub client_id: String,
    pub secret_set: bool,
    pub hd: Option<String>,
}

pub async fn google_client_status(pg: &PgPool, sb: &SecretBox) -> GoogleClientStatus {
    if let Some(client) = resolve_google_client(pg, sb).await {
        return GoogleClientStatus {
            configured: true,
            source: Some(client.source),
            client_id: client.client_id,
            secret_set: true,
            hd: client.hd,
        };
    }
    let stored = get_setting(pg, KEY, serde_json::Value::Object(Default::default())).await;
    GoogleClientStatus {
        configured: false,
        source: None,
        client_id: stored
            .get("clientId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        secret_set: stored
            .get("clientSecretEnc")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty()),
        hd: stored.get("hd").and_then(|v| v.as_str()).map(String::from),
    }
}

/// The PUT body's patch semantics — `None` is "key absent" (keep the stored
/// value), `Some(None)` is "explicit null" (clear it). The distinction is the
/// rotation flow: save a new id without re-entering the secret.
pub struct ClientConfigPatch {
    pub client_id: Option<String>,
    pub client_secret: Option<Option<String>>,
    pub hd: Option<Option<String>>,
}

/// Patch the stored client. The secret follows the house rule: a non-empty
/// string seals + stores it, null/'' clears it, absent leaves it untouched.
/// A client id that trims to nothing is the one refusal — the TS route lets
/// the throw escape (no catch, no boundary), which surfaces as an unstructured
/// 500; the caller maps the Err to the same.
pub async fn set_google_client_config(
    pg: &PgPool,
    sb: &SecretBox,
    patch: &ClientConfigPatch,
) -> Result<(), String> {
    let cur = get_setting(pg, KEY, serde_json::Value::Object(Default::default())).await;
    let stored_str = |k: &str| cur.get(k).and_then(|v| v.as_str());

    let next_client_id = match &patch.client_id {
        Some(v) => v.trim().to_string(),
        None => stored_str("clientId").unwrap_or_default().to_string(),
    };
    let next_enc = match &patch.client_secret {
        Some(Some(secret)) => {
            let trimmed = secret.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(sb.seal(trimmed).map_err(|e| format!("{e}"))?)
            }
        }
        // Present but null, or present-but-blank — either way: cleared.
        Some(None) => None,
        None => stored_str("clientSecretEnc").map(String::from),
    };
    let next_hd = match &patch.hd {
        Some(Some(hd)) => {
            let trimmed = hd.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(None) => None,
        None => stored_str("hd").map(String::from),
    };

    if next_client_id.is_empty() {
        return Err("a client id is required".into());
    }

    // StoredClient's literal order — the stored jsonb keeps it.
    let mut next = serde_json::Map::new();
    next.insert("clientId".into(), serde_json::Value::String(next_client_id));
    next.insert(
        "clientSecretEnc".into(),
        next_enc
            .map(serde_json::Value::String)
            .unwrap_or(Value::Null),
    );
    next.insert(
        "hd".into(),
        next_hd
            .map(serde_json::Value::String)
            .unwrap_or(Value::Null),
    );
    set_setting(pg, KEY, &serde_json::Value::Object(next))
        .await
        .map_err(|e| format!("{e}"))
}

/// Drop the stored record. Env credentials (if any) take over again —
/// clearing the Admin record never disables a working env deployment.
pub async fn clear_google_client_config(pg: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("delete from app_settings where key = $1")
        .bind(KEY)
        .execute(pg)
        .await
        .map(|_| ())
}

/// Flip the Admin UI's login switch.
pub async fn set_google_login_enabled(pg: &PgPool, on: bool) -> Result<(), sqlx::Error> {
    set_setting(pg, LOGIN_KEY, &serde_json::Value::Bool(on)).await
}

/// The env flag's view alone — `AUTH_GOOGLE_ENABLED === '1' || === 'true'`,
/// exact strings, no trimming (an operator who pinned login on in env keeps
/// it on; the UI toggle cannot turn a deliberate env policy back off).
pub fn google_login_pinned_by_env() -> bool {
    matches!(
        std::env::var("AUTH_GOOGLE_ENABLED").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Whether GOOGLE LOGIN is offered. Login is a policy decision, not a
/// credential decision: configuring a client must not by itself open a new
/// way into the instance. The switch is the Admin UI toggle (stored beside
/// the client) with the env pin winning towards ON — and either way a
/// resolvable client is still required: a toggle with no credential behind it
/// must never render a login button that cannot work.
pub async fn google_login_enabled(pg: &PgPool, sb: &SecretBox) -> bool {
    let toggled = get_setting(pg, LOGIN_KEY, serde_json::Value::Bool(false))
        .await
        .as_bool()
        == Some(true);
    (google_login_pinned_by_env() || toggled) && resolve_google_client(pg, sb).await.is_some()
}
