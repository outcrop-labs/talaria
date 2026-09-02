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
    let v = if stored.is_null() {
        fallback
    } else {
        stored
    };
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
