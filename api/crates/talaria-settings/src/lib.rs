// app_settings — the settings door. Everything configured at runtime lives in
// this one table as jsonb: guardrails, budgets, learned params, capabilities.

use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Read one setting; `fallback` when the key is absent or unreadable.
pub async fn get_setting(pg: &PgPool, key: &str, fallback: serde_json::Value) -> serde_json::Value {
    match sqlx::query_scalar::<_, serde_json::Value>(
        "select value from app_settings where key = $1",
    )
    .bind(key)
    .fetch_optional(pg)
    .await
    {
        Ok(Some(v)) => v,
        _ => fallback,
    }
}

// ── The hot-settings serve window ────────────────────────────────────────────
//
// A few keys sit on the per-completion hot path (guardrails, budgets, the
// unmetered/agent-loop key lists) and are written ONLY through `set_setting`
// by their admin routes. For those, this 15s window takes the checkout off
// the hot path; `set_setting` drops the key so an in-process write lands on
// the very next read.
//
// LAW: OPT-IN PER KEY, by name, at the call site. app_settings has writers
// that go around `set_setting` (approvals, gaps, and secret_health's
// setting-leaf clear write raw UPDATEs), and a key one of those touches
// must NEVER read through this window — a blanket caching of `get_setting`
// would serve those keys stale. secret_health's leaf clear can in principle
// name any key; for the opted-in keys that door is the one residual writer,
// and the TTL is its bound.

const HOT_TTL: Duration = Duration::from_secs(15);

fn hot_cache() -> &'static Mutex<HashMap<String, (Instant, serde_json::Value)>> {
    static C: OnceLock<Mutex<HashMap<String, (Instant, serde_json::Value)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The hot-path read: the named key's 15s serve window. Only call for a key
/// whose every writer is `set_setting` (see the law above) — the fallback
/// is served and cached exactly like a stored value, so an absent key costs
/// one checkout per window, not per completion.
pub async fn get_setting_hot(
    pg: &PgPool,
    key: &str,
    fallback: serde_json::Value,
) -> serde_json::Value {
    {
        let c = hot_cache().lock().expect("hot settings cache");
        if let Some((at, hit)) = c.get(key)
            && at.elapsed() < HOT_TTL
        {
            return hit.clone();
        }
    }
    // The fallback resolves through the plain read: an absent key must not
    // cache the CALLER's fallback object forever — a later caller with a
    // different fallback still gets its own.
    let stored = get_setting(pg, key, serde_json::Value::Null).await;
    let v = if stored.is_null() { fallback } else { stored };
    hot_cache()
        .lock()
        .expect("hot settings cache")
        .insert(key.to_string(), (Instant::now(), v.clone()));
    v
}

pub async fn set_setting(
    pg: &PgPool,
    key: &str,
    value: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into app_settings (key, value) values ($1, $2) \
         on conflict (key) do update set value = excluded.value, updated_at = now()",
    )
    .bind(key)
    .bind(value)
    .execute(pg)
    .await
    .map(|_| ())?;
    // The write is visible in-process immediately — every hot key's writer
    // is this function (the law above), so one drop covers them all.
    hot_cache().lock().expect("hot settings cache").remove(key);
    Ok(())
}

// ── The redacted view of a row that holds a sealed secret ────────────────────

/// A settings row as a CLIENT may see it: the row's own keys in their own
/// order, `keySealed` removed, and a derived `hasKey` appended last.
///
/// WHY IT LIVES HERE rather than beside either of its callers. Several
/// provider-config rows in this table follow one shape — an operator picks a
/// service, supplies a key, and the key is sealed into the row — and each of
/// them needs exactly this fold before the row reaches a panel. The reranker
/// grew it first and the decision-model port needed the same thing; a second
/// copy is how the two come to disagree about whether an explicit `null` key
/// counts as "a key is set" (it does not, and that is the whole reason `hasKey`
/// is derived from presence AND non-nullness rather than from presence alone).
///
/// A PASSTHROUGH, NOT A RESHAPE: the wire's key order is the jsonb row's order,
/// because the panels carry the row through verbatim rather than parsing it
/// onto a struct.
pub fn public_of(stored: serde_json::Value) -> serde_json::Value {
    let mut map = match stored {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let has_key = map.get("keySealed").is_some_and(|k| !k.is_null());
    map.remove("keySealed");
    map.insert("hasKey".into(), serde_json::json!(has_key));
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_public_view_drops_the_seal_keeps_the_rows_order_and_appends_haskey_last() {
        let pubv = public_of(json!({"provider": "x", "keySealed": "sealed", "model": "m"}));
        assert!(pubv.get("keySealed").is_none());
        let keys: Vec<&str> = pubv
            .as_object()
            .expect("an object")
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, vec!["provider", "model", "hasKey"]);
        assert_eq!(pubv["hasKey"], json!(true));
    }

    #[test]
    fn an_explicit_null_seal_is_not_a_key_which_is_the_whole_reason_haskey_is_derived() {
        assert_eq!(
            public_of(json!({"keySealed": null}))["hasKey"],
            json!(false)
        );
        assert_eq!(public_of(json!({}))["hasKey"], json!(false));
    }

    #[test]
    fn a_non_object_row_folds_to_an_empty_one_rather_than_panicking() {
        assert_eq!(public_of(json!("corrupt")), json!({"hasKey": false}));
    }
}
