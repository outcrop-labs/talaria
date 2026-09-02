// Per-user API keys for the Talaria LLM gateway. The secret is `tlk_<48
// hex>`, shown exactly once
// at mint time; only its sha256 is stored. Admins may always mint; other
// users need the models.mint-keys grant (the fine-grained permission the old
// can_mint_keys column backfilled into).

use crate::agent_auth::epoch_ms_to_iso;
use crate::auth::sha256_hex;
use crate::permissions::has_perm;
use sqlx::PgPool;

/// One key as /api/keys serves it, in wire order. The cap columns
/// are ::float8 on every select (bigint/numeric would arrive as strings) and
/// ride the wire as js_num Numbers so an integral cap prints "1000", not
/// Rust's "1000.0".
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmApiKey {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub spend_cap_tokens: Option<serde_json::Number>,
    pub spend_cap_usd: Option<serde_json::Number>,
    pub rate_limit_per_minute: Option<serde_json::Number>,
}

/// The columns behind LlmApiKey, in select order — (id, name, prefix, the
/// three epoch-ms timestamps, the three caps).
type KeyRow = (
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

const KEY_ROW: &str = "id::text, name, prefix, \
     (trunc(extract(epoch from created_at) * 1000))::bigint, \
     (trunc(extract(epoch from last_used_at) * 1000))::bigint, \
     (trunc(extract(epoch from revoked_at) * 1000))::bigint, \
     spend_cap_tokens::float8, spend_cap_usd::float8, rate_limit_per_minute::float8";

fn wire(row: KeyRow) -> LlmApiKey {
    LlmApiKey {
        id: row.0,
        name: row.1,
        prefix: row.2,
        created_at: epoch_ms_to_iso(row.3),
        last_used_at: row.4.map(epoch_ms_to_iso),
        revoked_at: row.5.map(epoch_ms_to_iso),
        spend_cap_tokens: row.6.map(crate::body::js_num),
        spend_cap_usd: row.7.map(crate::body::js_num),
        rate_limit_per_minute: row.8.map(crate::body::js_num),
    }
}

pub async fn can_mint_keys(pg: &PgPool, user_id: &str, role: &str) -> Result<bool, sqlx::Error> {
    has_perm(pg, user_id, role, "models.mint-keys").await
}

/// Mint a key: 24 random bytes as hex under the `tlk_` prefix, sha256 stored,
/// 12-char prefix for the list view. The secret crosses the wire exactly once
/// — here and nowhere else.
pub async fn mint_key(
    pg: &PgPool,
    user_id: &str,
    name: &str,
) -> Result<(LlmApiKey, String), sqlx::Error> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("system rng");
    let secret = format!("tlk_{}", hex(&bytes));
    let prefix = secret[..12].to_string();
    // AssertSqlSafe: the interpolation is this crate's KEY_ROW column list.
    let sql = format!(
        "insert into llm_api_keys (user_id, name, key_hash, prefix) \
         values ($1::uuid, $2, $3, $4) returning {KEY_ROW}"
    );
    let row: KeyRow = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(name)
        .bind(sha256_hex(&secret))
        .bind(&prefix)
        .fetch_one(pg)
        .await?;
    Ok((wire(row), secret))
}

pub async fn list_keys(pg: &PgPool, user_id: &str) -> Result<Vec<LlmApiKey>, sqlx::Error> {
    // AssertSqlSafe: the interpolation is this crate's KEY_ROW column list.
    let sql = format!(
        "select {KEY_ROW} from llm_api_keys where user_id = $1::uuid order by created_at desc"
    );
    let rows: Vec<KeyRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(wire).collect())
}

/// Revoke — scoped to the owner, and a no-op that still answers ok when the
/// id is nobody's (the route audits either way).
pub async fn revoke_key(pg: &PgPool, user_id: &str, key_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update llm_api_keys set revoked_at = now() \
         where id = $1::uuid and user_id = $2::uuid",
    )
    .bind(key_id)
    .bind(user_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// The self-imposed policy a key owner can set (#265). null = unlimited.
#[derive(Clone, Copy)]
pub struct KeyPolicy {
    pub spend_cap_tokens: Option<f64>,
    pub spend_cap_usd: Option<f64>,
    pub rate_limit_per_minute: Option<f64>,
}

/// finite and > 0, else None — 0 means the same as unlimited and is
/// normalized on write so the row never disagrees about which spelling means
/// "off".
fn or_null(v: Option<f64>) -> Option<f64> {
    match v {
        Some(v) if v.is_finite() && v > 0.0 => Some(v),
        _ => None,
    }
}

/// Set a key's policy. Scoped to the owner and to LIVE keys; false = no such
/// key (theirs).
pub async fn set_key_policy(
    pg: &PgPool,
    user_id: &str,
    key_id: &str,
    policy: KeyPolicy,
) -> Result<bool, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "update llm_api_keys set \
           spend_cap_tokens = $3, spend_cap_usd = $4, rate_limit_per_minute = $5 \
         where id = $1::uuid and user_id = $2::uuid and revoked_at is null \
         returning id::text",
    )
    .bind(key_id)
    .bind(user_id)
    .bind(or_null(policy.spend_cap_tokens))
    .bind(or_null(policy.spend_cap_usd))
    .bind(or_null(policy.rate_limit_per_minute))
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_mint_as_tlk_plus_forty_eight_hex() {
        // tlk_ + 48 hex chars; the list
        // view shows the first 12 — "tlk_" plus 8 hex of secret.
        let secret = format!("tlk_{}", hex(&(0u8..24).collect::<Vec<u8>>()));
        assert_eq!(secret.len(), 52);
        assert!(secret[4..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(&secret[..12], "tlk_00010203");
        // And the stored form is the sha256 hex of exactly this string.
        assert_eq!(sha256_hex(&secret).len(), 64);
    }

    #[test]
    fn or_null_normalizes_zero_and_keeps_positive() {
        assert_eq!(or_null(Some(100.0)), Some(100.0));
        assert_eq!(or_null(Some(0.5)), Some(0.5));
        assert_eq!(or_null(Some(0.0)), None); // 0 = unlimited, on purpose
        assert_eq!(or_null(Some(-1.0)), None);
        assert_eq!(or_null(None), None);
    }
}
