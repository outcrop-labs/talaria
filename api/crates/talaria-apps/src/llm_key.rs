// THE APP'S GATEWAY KEY — minted, sealed, rotated and revoked here. The
// vocabulary (the `app:<slug>` name, the `app_llm_keys` record, the headers)
// is talaria-gateway's `app_keys`; the relay (llm_chat.rs) judges calls on it.
//
// Every model call the app host makes for an app goes to the relay on this
// key, so the app is metered, budgeted and priced the way every other caller
// is. The host asks for it over the internal route
// (`POST /api/internal/apps/<slug>/llm-key`) and keeps it in memory; app code
// never sees it.
//
//   mint     on the host's first ask while the app is enabled: one `tlk_` key
//            named `app:<slug>`, owned by the instance's first admin (as the
//            fleet's own keys are), its secret sealed into `app_llm_keys`
//            (docs/ENCRYPTION.md) beside the key's id
//   keep     a later ask answers the same secret while it still opens and
//            the key row is live — a host restart is not a rotation
//   rotate   a sealed secret that no longer opens (a lost root), a key row
//            revoked or replaced, a re-enable after a disable: the old key is
//            revoked and a new one minted
//   revoke   on disable (`set_app_enabled`), and so on uninstall, which
//            disables first; and in `reconcile_app_llm_keys`, which the api
//            runs at boot and the host's resync runs after the host turns a
//            broken app off itself — every key of an app that is not enabled
//            stops authenticating, and its record goes
//
// The record, not the name, is what the relay trusts: a key named
// `app:<slug>` whose id the record does not hold is refused.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_gateway::app_keys::{APP_KEY_PREFIX, APP_KEYS_SETTING, app_key_name, app_slug_of_key};
use talaria_gateway::settings::{get_setting, set_setting};
use talaria_secretbox::SecretBox;

/// Why no key could be handed out.
#[derive(Debug)]
pub enum AppKeyError {
    /// The app is not enabled: it runs no code, so it makes no calls.
    NotEnabled,
    /// The instance has no user yet to own a key (a fresh install before
    /// the first login).
    NoOwner,
    /// The database or the secretbox failed.
    Store(String),
}

impl std::fmt::Display for AppKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppKeyError::NotEnabled => f.write_str("the app is not enabled"),
            AppKeyError::NoOwner => f.write_str("no user exists yet to own the app's gateway key"),
            AppKeyError::Store(e) => f.write_str(e),
        }
    }
}

fn store<E: std::fmt::Display>(what: &'static str) -> impl FnOnce(E) -> AppKeyError {
    move |e| AppKeyError::Store(format!("{what}: {e}"))
}

/// One ask at a time: two concurrent first asks would each mint, and the
/// second would revoke the key the first just handed out.
fn ensure_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// The record's entry for `slug`: the key id and the sealed secret, when
/// both are there.
fn entry_of<'a>(record: &'a Value, slug: &str) -> Option<(&'a str, &'a str)> {
    let e = record.get(slug)?;
    let id = e.get("keyId")?.as_str().filter(|s| !s.is_empty())?;
    let sealed = e.get("sealed")?.as_str().filter(|s| !s.is_empty())?;
    Some((id, sealed))
}

/// The record with `slug`'s entry set (or removed, for None). Pure.
fn with_entry(record: &Value, slug: &str, entry: Option<Value>) -> Value {
    let mut map = record.as_object().cloned().unwrap_or_default();
    match entry {
        Some(e) => {
            map.insert(slug.to_string(), e);
        }
        None => {
            map.remove(slug);
        }
    }
    Value::Object(map)
}

/// The app's gateway secret: the one the record holds while it still opens
/// and its key row is live, else a freshly minted one (any older key of the
/// app's revoked first). Refused for an app that is not enabled.
pub async fn ensure_app_llm_key(
    pg: &PgPool,
    sb: &SecretBox,
    slug: &str,
) -> Result<String, AppKeyError> {
    if !talaria_users::slug_ok(slug) {
        return Err(AppKeyError::NotEnabled);
    }
    let _one = ensure_lock().lock().await;
    if !crate::enabled_app_slugs(pg).await.iter().any(|s| s == slug) {
        return Err(AppKeyError::NotEnabled);
    }
    let name = app_key_name(slug);
    let record = get_setting(pg, APP_KEYS_SETTING, json!({})).await;
    if let Some((key_id, sealed)) = entry_of(&record, slug)
        && let Ok(secret) = sb.open(sealed)
    {
        let live: Option<(i32,)> = sqlx::query_as(
            "select 1 from llm_api_keys \
              where id::text = $1 and name = $2 and revoked_at is null and key_hash = $3",
        )
        .bind(key_id)
        .bind(&name)
        .bind(talaria_auth::sha256_hex(&secret))
        .fetch_optional(pg)
        .await
        .map_err(store("app key live-check"))?;
        if live.is_some() {
            return Ok(secret);
        }
    }

    let owner: Option<(String,)> = sqlx::query_as(
        "select id::text from users order by (role = 'admin') desc, created_at asc limit 1",
    )
    .fetch_optional(pg)
    .await
    .map_err(store("app key owner lookup"))?;
    let Some((owner_id,)) = owner else {
        return Err(AppKeyError::NoOwner);
    };
    // Whatever the app held before stops authenticating before its
    // replacement exists.
    revoke_keys_named(pg, &name)
        .await
        .map_err(store("stale app key revoke"))?;
    let (key, secret) = talaria_llm_keys::mint_key(pg, &owner_id, &name)
        .await
        .map_err(store("app key mint"))?;
    // A key the record does not hold authenticates as nobody's app (the relay
    // refuses it), and the next ask revokes it; still, one that could not be
    // sealed or recorded is revoked now rather than left live until then.
    let recorded = match sb.seal(&secret) {
        Ok(sealed) => {
            let next = with_entry(
                &record,
                slug,
                Some(json!({ "keyId": key.id, "sealed": sealed })),
            );
            set_setting(pg, APP_KEYS_SETTING, &next)
                .await
                .map_err(store("app key record"))
        }
        Err(e) => Err(store("app key seal")(e)),
    };
    if let Err(e) = recorded {
        let _ = revoke_keys_named(pg, &name).await;
        return Err(e);
    }
    tracing::info!(
        "[apps] {slug}: minted the app's gateway key ({})",
        key.prefix
    );
    Ok(secret)
}

/// Revoke every live key named `name`, and say so to the identity cache so
/// the revocation lands on the next call. Answers how many it revoked.
async fn revoke_keys_named(pg: &PgPool, name: &str) -> Result<u64, sqlx::Error> {
    let n = sqlx::query(
        "update llm_api_keys set revoked_at = now() where name = $1 and revoked_at is null",
    )
    .bind(name)
    .execute(pg)
    .await?
    .rows_affected();
    if n > 0 {
        talaria_auth::reset_identity_cache();
    }
    Ok(n)
}

/// Revoke the app's gateway key and drop its record. A disable or an
/// uninstall: an app that runs no code makes no calls, and the host's copy
/// of the key stops authenticating now.
pub async fn revoke_app_llm_key(pg: &PgPool, slug: &str) -> Result<(), sqlx::Error> {
    let _one = ensure_lock().lock().await;
    revoke_keys_named(pg, &app_key_name(slug)).await?;
    let record = get_setting(pg, APP_KEYS_SETTING, json!({})).await;
    if record.get(slug).is_some() {
        set_setting(pg, APP_KEYS_SETTING, &with_entry(&record, slug, None)).await?;
    }
    Ok(())
}

/// The slugs whose keys must go: every app named by a live key or by the
/// record that is not in the enabled set. Pure.
pub fn stale_app_key_slugs(
    enabled: &BTreeSet<String>,
    live_key_names: &[String],
    record: &Value,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for name in live_key_names {
        if let Some(slug) = app_slug_of_key(name)
            && !enabled.contains(slug)
        {
            out.insert(slug.to_string());
        }
    }
    if let Some(map) = record.as_object() {
        for slug in map.keys() {
            if !enabled.contains(slug) {
                out.insert(slug.clone());
            }
        }
    }
    out
}

/// Revoke the key of every app that is not enabled, whoever turned it off —
/// the admin route revokes as it disables, but the app host turns a broken
/// app off by writing the enabled set itself, and a key can outlive an api
/// that was down. Answers the slugs whose keys went.
pub async fn reconcile_app_llm_keys(pg: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let enabled: BTreeSet<String> = crate::enabled_app_slugs(pg).await.into_iter().collect();
    let live: Vec<(String,)> =
        sqlx::query_as("select name from llm_api_keys where name like $1 and revoked_at is null")
            .bind(format!("{APP_KEY_PREFIX}%"))
            .fetch_all(pg)
            .await?;
    let live: Vec<String> = live.into_iter().map(|(n,)| n).collect();
    let record = get_setting(pg, APP_KEYS_SETTING, json!({})).await;
    let stale = stale_app_key_slugs(&enabled, &live, &record);
    for slug in &stale {
        revoke_app_llm_key(pg, slug).await?;
        tracing::info!("[apps] {slug}: revoked the gateway key of an app that is not enabled");
    }
    Ok(stale.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_needs_both_its_id_and_its_sealed_secret() {
        let record = json!({
            "crm": { "keyId": "k1", "sealed": "v2:1:a:b:c" },
            "half": { "keyId": "k2" },
            "blank": { "keyId": "", "sealed": "v2:x" },
        });
        assert_eq!(entry_of(&record, "crm"), Some(("k1", "v2:1:a:b:c")));
        assert_eq!(entry_of(&record, "half"), None);
        assert_eq!(entry_of(&record, "blank"), None);
        assert_eq!(entry_of(&record, "gone"), None);
        assert_eq!(entry_of(&json!(null), "crm"), None);
    }

    #[test]
    fn an_entry_is_set_and_removed_without_touching_the_others() {
        let record = json!({ "crm": { "keyId": "k1" }, "billing": { "keyId": "k2" } });
        let set = with_entry(&record, "crm", Some(json!({ "keyId": "k3" })));
        assert_eq!(set["crm"]["keyId"], "k3");
        assert_eq!(set["billing"]["keyId"], "k2");
        let gone = with_entry(&record, "crm", None);
        assert!(gone.get("crm").is_none());
        assert_eq!(gone["billing"]["keyId"], "k2");
        // A record that is not an object starts over as one.
        assert_eq!(
            with_entry(&json!(null), "crm", Some(json!({}))),
            json!({ "crm": {} })
        );
    }

    #[test]
    fn a_disabled_apps_key_is_stale_wherever_it_is_named() {
        let enabled: BTreeSet<String> = ["crm".to_string()].into();
        let live = vec![
            "app:crm".to_string(),
            "app:billing".to_string(),
            "agent:crm".to_string(),
            "my-own-key".to_string(),
        ];
        let record = json!({ "crm": {}, "notes": {} });
        let stale = stale_app_key_slugs(&enabled, &live, &record);
        assert_eq!(
            stale.into_iter().collect::<Vec<_>>(),
            vec!["billing".to_string(), "notes".to_string()],
            "a live key and a record entry both count; another kind of key never does"
        );
        assert!(
            stale_app_key_slugs(&enabled, &["app:crm".to_string()], &json!({ "crm": {} }))
                .is_empty()
        );
    }
}
