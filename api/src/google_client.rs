// The Google OAuth CLIENT credentials — port of the read side of
// ui/src/server/google/client-config.ts. The record lives in app_settings
// (`google_oauth_client`) with the secret SEALED (secretbox); env
// (AUTH_GOOGLE_CLIENT_ID/_SECRET) remains the fallback, and the record wins
// only when it is complete — a half-saved record never takes over from
// working env credentials, so exactly one client is ever active. The admin
// write paths (set/clear/status) port with the admin surfaces in batch 3.

use crate::gateway::settings::get_setting;
use crate::secretbox::SecretBox;
use sqlx::PgPool;

const LOGIN_KEY: &str = "google_login_enabled";

pub struct GoogleClient {
    pub client_id: String,
    pub client_secret: String,
    pub hd: Option<String>,
}

/// The one client every Google flow uses, or None when nothing usable is
/// configured anywhere. DB record wins when complete; env is the fallback.
pub async fn resolve_google_client(pg: &PgPool, sb: &SecretBox) -> Option<GoogleClient> {
    let stored = get_setting(
        pg,
        "google_oauth_client",
        serde_json::Value::Object(Default::default()),
    )
    .await;
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
        });
    }
    None
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
