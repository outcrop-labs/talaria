// app_settings — the settings door. Everything configured at runtime lives in
// this one table as jsonb: guardrails, budgets, learned params, capabilities.

use sqlx::PgPool;

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
    .map(|_| ())
}
