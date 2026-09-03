// One-shot rotation of the data-encryption key (DEK). Generates a fresh
// 256-bit DEK and re-encrypts EVERY stored secret under it in a single
// transaction — the
// operator rotates once and all provider keys, agent secrets, and OAuth
// tokens move to the new key automatically. Optionally re-derives the KEK
// from new root material at the same time (the wrapped DEK then moves under
// the new root).
//
// Every cipher column in the database is listed here; add new ones as secrets
// are introduced so rotation stays complete.

use crate::secretbox::{SecretBox, new_dek};
use sqlx::{PgPool, Row};

/// Every table+column holding a secretbox ciphertext, with its primary key.
/// Identifiers here are all code-defined constants, never user input — that
/// is what makes the format! strings below safe.
const CIPHER_TARGETS: &[(&str, &str, &[&str])] = &[
    ("llm_endpoints", "api_key_cipher", &["id"]),
    ("agent_secrets", "value_enc", &["agent_id", "name"]),
    ("google_connections", "access_token_enc", &["user_id"]),
    ("google_connections", "refresh_token_enc", &["user_id"]),
    ("google_org_connection", "access_token_enc", &["id"]),
    ("google_org_connection", "refresh_token_enc", &["id"]),
];

#[derive(serde::Serialize)]
pub struct RotationResult {
    pub version: u32,
    pub reencrypted: i64,
    #[serde(rename = "rootRewrapped")]
    pub root_rewrapped: bool,
}

/// The pk comparison the sweep's update addresses rows by. The pk values were
/// READ as `"col"::text` (dynamic arity, one code path for every table), so
/// they compare as text = text. The pk columns are NOT all text — uuid, uuid,
/// integer across CIPHER_TARGETS — and a bare `"col" = $N` with a text bind is
/// the port's crash class: sqlx declares the bind's wire type from the Rust
/// value, prepare resolves `uuid = text`, and the operator does not exist.
/// Rotation is a full-table sweep, so the column-side cast costs nothing.
fn pk_where(pk: &[&str]) -> String {
    pk.iter()
        .enumerate()
        .map(|(i, c)| format!("\"{c}\"::text = ${}", i + 2))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Rotate the DEK and re-encrypt all secrets. `new_root_material` also moves
/// the wrapped DEK under a new operator secret (KEK) in the same pass.
///
/// Returns the NEW key set alongside the counts: the caller installs it into
/// the process only after this returns Ok — the DB transaction commits, then
/// the install — so a failure here leaves the in-memory keys and the database
/// in their old, consistent state.
pub async fn rotate_secrets(
    pg: &PgPool,
    sb: &SecretBox,
    new_root_material: Option<&str>,
) -> Result<(SecretBox, RotationResult), String> {
    let fresh = new_dek().map_err(|e| e.to_string())?;
    let next_version = sb.current_key_version().map_err(|e| e.to_string())? + 1;
    let mut reencrypted: i64 = 0;

    let mut tx = pg.begin().await.map_err(|e| e.to_string())?;

    for (table, column, pk) in CIPHER_TARGETS {
        let cols = pk
            .iter()
            .map(|c| format!("\"{c}\"::text"))
            .chain([format!("\"{column}\"")])
            .collect::<Vec<_>>()
            .join(", ");
        // AssertSqlSafe: every identifier below is a code-defined constant
        // from CIPHER_TARGETS.
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "select {cols} from \"{table}\" where \"{column}\" is not null"
        )))
        .fetch_all(tx.as_mut())
        .await
        .map_err(|e| e.to_string())?;

        for row in &rows {
            // The pk columns sit at 0..pk.len() as text, the cipher last —
            // dynamic arity (1 or 2 pk columns per target) without a tuple.
            let cipher: String = row
                .try_get(pk.len())
                .map_err(|e| format!("{table}.{column}: {e}"))?;
            let plain = sb.open(&cipher).map_err(|e| e.to_string())?;
            let resealed = sb
                .seal_with(&fresh, next_version, &plain)
                .map_err(|e| e.to_string())?;
            let where_clause = pk_where(pk);
            let mut q = sqlx::query(sqlx::AssertSqlSafe(format!(
                "update \"{table}\" set \"{column}\" = $1 where {where_clause}"
            )))
            .bind(&resealed);
            for i in 0..pk.len() {
                let v: String = row.try_get(i).map_err(|e| e.to_string())?;
                q = q.bind(v);
            }
            q.execute(tx.as_mut()).await.map_err(|e| e.to_string())?;
            reencrypted += 1;
        }
    }

    // If the root secret is changing, re-wrap every RETAINED key version under
    // the new KEK so old versions stay unwrappable (old ciphertext keeps working).
    if let Some(material) = new_root_material {
        for v in sb.loaded_versions() {
            let wrapped = sb.rewrap_version(v, material).map_err(|e| e.to_string())?;
            sqlx::query("update secret_keys set wrapped_dek = $1 where version = $2")
                .bind(&wrapped)
                .bind(v as i32)
                .execute(tx.as_mut())
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    // Publish the new active version — prior versions are KEPT (marked
    // inactive) so their ciphertext still decrypts.
    sqlx::query("update secret_keys set active = false where active")
        .execute(tx.as_mut())
        .await
        .map_err(|e| e.to_string())?;
    let wrapped_new = sb
        .wrap_dek_for(&fresh, new_root_material)
        .map_err(|e| e.to_string())?;
    sqlx::query("insert into secret_keys (version, wrapped_dek, active) values ($1, $2, true)")
        .bind(next_version as i32)
        .bind(&wrapped_new)
        .execute(tx.as_mut())
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;

    Ok((
        sb.installed(fresh, next_version, new_root_material),
        RotationResult {
            version: next_version,
            reencrypted,
            root_rewrapped: new_root_material.is_some(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pk column compares as ::text, whatever its real type — the sweep
    /// reads them as ::text and binds them back as strings, so the cast is what
    /// keeps `uuid = text` (and `integer = text`) out of prepare.
    #[test]
    fn pk_comparisons_stay_text_typed() {
        assert_eq!(pk_where(&["id"]), r#""id"::text = $2"#);
        assert_eq!(
            pk_where(&["agent_id", "name"]),
            r#""agent_id"::text = $2 and "name"::text = $3"#
        );
    }
}
