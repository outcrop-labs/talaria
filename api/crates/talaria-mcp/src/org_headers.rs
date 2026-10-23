// ORG-LEVEL MCP HEADERS, SEALED AT REST.
//
// An admin types an API key into a server's Credentials form; that key is a
// secret like any other and lives SecretBox-sealed in
// `mcp_servers.headers_sealed` (a JSON object, sealed whole). The legacy
// plaintext `headers` jsonb column is `'{}'` on every row this module has
// written; a row still holding plaintext is a pre-sealing row, read as-is and
// sealed by `seal_legacy_headers` once a process with a SecretBox has been up
// past `legacy_sealing_grace` (an old slot still draining reads only the
// plaintext column, so the sweep waits it out).
//
// THIS FILE IS THE ONLY PLACE THAT OPENS THEM. Every reader — the gateway's
// upstream injection, discovery, the platform's own tool calls — goes
// through `open_headers`, and every writer through `write_headers`, so there
// is exactly one spelling of "which value wins" and one place a key rotation
// or an audit has to look. Header VALUES never leave the api: the wire
// carries `header_keys` only.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use sqlx::PgPool;
use talaria_secretbox::SecretBox;

use crate::registry::McpServer;

/// The decrypted org headers of one server. Sealed value first; a legacy
/// plaintext key still on the row overlays it (a row only carries both when
/// an older binary wrote plaintext during a rolling deploy — its write is the
/// newer one). An unreadable seal is an error, never an empty map: silently
/// speaking upstream without the key reads as "the key is wrong" to the
/// admin, while the real fault is the encryption key.
pub fn open_headers(sb: &SecretBox, server: &McpServer) -> Result<Map<String, Value>, String> {
    open_parts(sb, server.headers_sealed.as_deref(), &server.headers_plain)
}

/// Just the key names — what the wire, the audit log and the credentials
/// form may know.
pub fn header_keys(sb: &SecretBox, server: &McpServer) -> Result<Vec<String>, String> {
    Ok(open_headers(sb, server)?.keys().cloned().collect())
}

/// The headers as the ordered string pairs an HTTP request is built from —
/// non-string values (never written by the routes, but jsonb allows them)
/// are skipped, as they always were.
pub fn header_pairs(headers: &Map<String, Value>) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
        .collect()
}

fn open_parts(
    sb: &SecretBox,
    sealed: Option<&str>,
    plain: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let mut out = match sealed.filter(|s| !s.is_empty()) {
        Some(token) => {
            let text = sb.open(token).map_err(|e| {
                format!(
                    "the server's stored headers could not be opened ({e}); set them again under \
                     Credentials on the server card"
                )
            })?;
            match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(m)) => m,
                _ => return Err("the server's stored headers are not a JSON object".into()),
            }
        }
        None => Map::new(),
    };
    for (k, v) in plain {
        out.insert(k.clone(), v.clone());
    }
    Ok(out)
}

/// `mergeHeaders`: a string value sets that key, `null` removes it, every
/// other stored key stays. Any other value type is ignored, exactly as the
/// jsonb `(headers || set) - removed` expression this replaced ignored it.
pub fn apply_merge(current: &Map<String, Value>, merge: &Map<String, Value>) -> Map<String, Value> {
    let mut out = current.clone();
    for (k, v) in merge {
        match v {
            Value::String(_) => {
                out.insert(k.clone(), v.clone());
            }
            Value::Null => {
                out.remove(k);
            }
            _ => {}
        }
    }
    out
}

/// Seal a header map for the row. An empty map stores SQL NULL — "no org
/// headers" has one spelling.
pub fn seal_headers(
    sb: &SecretBox,
    headers: &Map<String, Value>,
) -> Result<Option<String>, String> {
    if headers.is_empty() {
        return Ok(None);
    }
    sb.seal(&Value::Object(headers.clone()).to_string())
        .map(Some)
        .map_err(|e| format!("header seal failed: {e}"))
}

/// The two write shapes a PUT can carry: replace the whole map, and/or merge
/// keys into it (replace first, then merge — the route's historical order).
pub struct HeaderWrite<'a> {
    pub replace: Option<&'a Map<String, Value>>,
    pub merge: Option<&'a Map<String, Value>>,
}

/// Apply a header write to one row: open what is stored, apply the change,
/// re-seal, and write `headers_sealed` with the plaintext column emptied — in
/// one transaction holding the row lock, so two concurrent credential saves
/// cannot each read the old map and lose the other's key (the old jsonb
/// expression was atomic; this keeps that property).
pub async fn write_headers(
    pg: &PgPool,
    sb: &SecretBox,
    id: &str,
    write: &HeaderWrite<'_>,
) -> Result<(), String> {
    if write.replace.is_none() && write.merge.is_none() {
        return Ok(());
    }
    let mut tx = pg.begin().await.map_err(|e| e.to_string())?;
    let row: Option<(Value, Option<String>)> = sqlx::query_as(
        "select headers, headers_sealed from mcp_servers where id::text = $1 for update",
    )
    .bind(id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(|e| e.to_string())?;
    let Some((plain, sealed)) = row else {
        return Ok(()); // the row is gone — a no-op, like every other patch on it
    };
    let plain = plain.as_object().cloned().unwrap_or_default();
    let mut next = match write.replace {
        Some(replace) => replace.clone(),
        None => current_for_rewrite(sb, id, sealed.as_deref(), &plain),
    };
    if let Some(merge) = write.merge {
        next = apply_merge(&next, merge);
    }
    let sealed_next = seal_headers(sb, &next)?;
    sqlx::query(
        "update mcp_servers set headers_sealed = $2, headers = '{}'::jsonb, updated_at = now() \
         where id::text = $1",
    )
    .bind(id)
    .bind(&sealed_next)
    .execute(tx.as_mut())
    .await
    .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// What a merge starts from. Normally the opened row; but a seal this
/// process cannot open (the DB was restored onto other key material, or the
/// row was copied from another instance) holds nothing anyone can use — every
/// reader already fails on it, and the wire lists no keys, so the card offers
/// "Set credentials…". Failing the merge too would make that offer a dead
/// end: the credentials form only ever merges. So the unreadable seal is
/// dropped here, and only here — the admin's save is the decision to replace
/// it — keeping any plaintext still on the row, and the log names the
/// server.
fn current_for_rewrite(
    sb: &SecretBox,
    id: &str,
    sealed: Option<&str>,
    plain: &Map<String, Value>,
) -> Map<String, Value> {
    open_parts(sb, sealed, plain).unwrap_or_else(|e| {
        tracing::warn!(
            "[mcp] server {id}: replacing org headers that could not be opened ({e}); \
             any key not set again in this save is gone"
        );
        plain.clone()
    })
}

/// How long after this process started the legacy sweep waits before it
/// empties any plaintext column. A self-update runs the old slot beside the
/// new one — the new slot boots, passes its health gate, takes the traffic,
/// and the old one keeps serving through its drain
/// (`TALARIA_ROLL_DRAIN_SECONDS`). The old binary reads ONLY the plaintext
/// column: seal a row under it and its gateway calls go upstream without the
/// key, and its `mergeHeaders` null cannot remove a key that now lives in the
/// seal. Waiting out the drain plus a margin for the health gate means the
/// sweep only runs once no old binary is left to read the column. Until
/// then nothing is lost: `open_headers` reads the plaintext as-is.
pub fn legacy_sealing_grace() -> Duration {
    Duration::from_millis(talaria_update_layout::roll_drain_ms()) + Duration::from_secs(180)
}

/// When this process started counting the grace — set by the boot spawn,
/// or by the first registry read if that comes first.
static GRACE_FROM: OnceLock<Instant> = OnceLock::new();

fn grace_started() -> Instant {
    *GRACE_FROM.get_or_init(Instant::now)
}

/// The registry read's sweep: a no-op until the grace has passed, then the
/// same idempotent pass as boot — so a row restored from an old backup is
/// sealed the next time an admin looks, without ever racing an old slot.
pub async fn seal_legacy_headers_when_settled(pg: &PgPool, sb: &SecretBox) {
    if grace_started().elapsed() >= legacy_sealing_grace() {
        log_sweep(seal_legacy_headers(pg, sb).await);
    }
}

/// SEAL EVERY ROW STILL HOLDING PLAINTEXT. Idempotent and safe to run from
/// every instance at once: each row's update is conditioned on the exact
/// plaintext it read, so a second sealer (or an admin save in between) finds
/// `headers = '{}'` and matches nothing — never a double-seal, never a lost
/// write. Returns how many rows this call sealed.
pub async fn seal_legacy_headers(pg: &PgPool, sb: &SecretBox) -> Result<u64, String> {
    let rows: Vec<(String, Value, Option<String>)> = sqlx::query_as(
        "select id::text, headers, headers_sealed from mcp_servers where headers <> '{}'::jsonb",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| e.to_string())?;
    let mut sealed_rows = 0u64;
    for (id, plain, sealed) in rows {
        let merged = match open_parts(
            sb,
            sealed.as_deref(),
            &plain.as_object().cloned().unwrap_or_default(),
        ) {
            Ok(m) => m,
            Err(e) => {
                // Leave the row exactly as it is: the plaintext keeps working
                // through open_headers, and nothing here may destroy it.
                tracing::warn!("[mcp] could not seal the headers of server {id}: {e}");
                continue;
            }
        };
        let sealed_next = seal_headers(sb, &merged)?;
        let done = sqlx::query(
            "update mcp_servers set headers_sealed = $2, headers = '{}'::jsonb, updated_at = now() \
             where id::text = $1 and headers = $3",
        )
        .bind(&id)
        .bind(&sealed_next)
        .bind(&plain)
        .execute(pg)
        .await
        .map_err(|e| e.to_string())?;
        sealed_rows += done.rows_affected();
    }
    Ok(sealed_rows)
}

/// The boot one-shot: wait out `legacy_sealing_grace`, then
/// `seal_legacy_headers` — owning its arguments so it can ride a
/// `tokio::spawn`, saying what it did.
pub async fn seal_legacy_headers_after_grace(pg: PgPool, sb: SecretBox) {
    let wait = legacy_sealing_grace().saturating_sub(grace_started().elapsed());
    tokio::time::sleep(wait).await;
    log_sweep(seal_legacy_headers(&pg, &sb).await);
}

fn log_sweep(outcome: Result<u64, String>) {
    match outcome {
        Ok(0) => {}
        Ok(n) => tracing::info!("[mcp] sealed the plaintext org headers of {n} server(s)"),
        Err(e) => tracing::warn!("[mcp] legacy header sealing failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn test_box() -> SecretBox {
        let kek = talaria_secretbox::derive_kek("org-headers-test");
        let dek = talaria_secretbox::new_dek().expect("entropy");
        SecretBox::from_parts(kek, HashMap::from([(1u32, dek)]), Some(1))
    }

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().cloned().expect("test maps are objects")
    }

    fn server_with(sealed: Option<String>, plain: Map<String, Value>) -> McpServer {
        McpServer {
            id: "00000000-0000-0000-0000-000000000001".into(),
            name: "exa".into(),
            label: "Exa".into(),
            description: None,
            url: "https://mcp.exa.ai/mcp".into(),
            headers_plain: plain,
            headers_sealed: sealed,
            timeout_secs: None,
            enabled: true,
            all_agents: false,
            auth_mode: "org".into(),
            tools: json!([]),
            tools_refreshed_ms: None,
            required_headers: json!([]),
            oauth_enabled: false,
            builtin: false,
            app_slug: None,
            created_by: None,
            created_ms: 0,
            package: None,
            env_enc: None,
            last_probe: None,
            app_off: None,
        }
    }

    #[test]
    fn seal_then_open_round_trips_through_the_helper() {
        let sb = test_box();
        let headers = map(json!({ "X-Api-Key": "sk-live-123", "X-Org": "acme" }));
        let sealed = seal_headers(&sb, &headers)
            .unwrap()
            .expect("non-empty seals");
        assert!(
            !sealed.contains("sk-live-123"),
            "the sealed token must not carry the value in the clear"
        );
        let server = server_with(Some(sealed), Map::new());
        assert_eq!(open_headers(&sb, &server).unwrap(), headers);
        let mut keys = header_keys(&sb, &server).unwrap();
        keys.sort();
        assert_eq!(keys, vec!["X-Api-Key".to_string(), "X-Org".to_string()]);
    }

    #[test]
    fn an_empty_map_seals_to_null_and_opens_empty() {
        let sb = test_box();
        assert_eq!(seal_headers(&sb, &Map::new()).unwrap(), None);
        assert!(
            open_headers(&sb, &server_with(None, Map::new()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_legacy_plaintext_row_reads_until_it_is_sealed() {
        let sb = test_box();
        let plain = map(json!({ "Authorization": "Bearer legacy" }));
        assert_eq!(
            open_headers(&sb, &server_with(None, plain.clone())).unwrap(),
            plain
        );
        // Both present (an older binary wrote during a rolling deploy): the
        // plaintext key is the newer write and wins; sealed-only keys stay.
        let sealed = seal_headers(
            &sb,
            &map(json!({ "Authorization": "Bearer old", "X-Org": "acme" })),
        )
        .unwrap();
        let got = open_headers(&sb, &server_with(sealed, plain)).unwrap();
        assert_eq!(
            got,
            map(json!({ "Authorization": "Bearer legacy", "X-Org": "acme" }))
        );
    }

    #[test]
    fn an_unreadable_seal_is_an_error_not_an_empty_map() {
        let sb = test_box();
        let other = test_box(); // a different data key
        let sealed = seal_headers(&other, &map(json!({ "X-Api-Key": "sk-secret-zzz" }))).unwrap();
        let err = open_headers(&sb, &server_with(sealed, Map::new())).unwrap_err();
        assert!(err.contains("could not be opened"), "{err}");
        assert!(
            !err.contains("sk-secret-zzz"),
            "no value in the error: {err}"
        );
    }

    #[test]
    fn merge_sets_strings_removes_nulls_and_keeps_the_rest() {
        let sb = test_box();
        // The merge runs against what the SEALED value opens to — the same
        // path write_headers takes.
        let sealed = seal_headers(
            &sb,
            &map(json!({ "X-Api-Key": "old", "X-Keep": "kept", "X-Drop": "gone" })),
        )
        .unwrap();
        let current = open_headers(&sb, &server_with(sealed, Map::new())).unwrap();
        let merged = apply_merge(
            &current,
            &map(json!({ "X-Api-Key": "new", "X-Drop": null, "X-Added": "a", "X-Weird": 7 })),
        );
        assert_eq!(
            merged,
            map(json!({ "X-Api-Key": "new", "X-Keep": "kept", "X-Added": "a" })),
            "set overwrites, null removes, untouched keys stay, non-strings are ignored"
        );
        // Removing a key that is not stored is a no-op, not an error.
        assert_eq!(
            apply_merge(&merged, &map(json!({ "X-Nope": null }))),
            merged
        );
        // Re-sealed and re-opened, the merge result survives intact.
        let resealed = seal_headers(&sb, &merged).unwrap();
        assert_eq!(
            open_headers(&sb, &server_with(resealed, Map::new())).unwrap(),
            merged
        );
        // Removing every key seals to NULL — the empty spelling.
        let emptied = apply_merge(
            &merged,
            &map(json!({ "X-Api-Key": null, "X-Keep": null, "X-Added": null })),
        );
        assert_eq!(seal_headers(&sb, &emptied).unwrap(), None);
    }

    #[test]
    fn a_merge_over_an_unreadable_seal_replaces_it_instead_of_failing() {
        let sb = test_box();
        let other = test_box(); // the seal was written under key material we lack
        let sealed = seal_headers(&other, &map(json!({ "X-Api-Key": "lost" })))
            .unwrap()
            .unwrap();
        // Readers still refuse it — speaking upstream keyless is never silent.
        assert!(open_parts(&sb, Some(&sealed), &Map::new()).is_err());
        // The credentials form's merge starts from nothing usable instead of
        // failing, so "Set credentials…" actually recovers the server.
        let start = current_for_rewrite(&sb, "id", Some(&sealed), &Map::new());
        assert!(start.is_empty());
        let next = apply_merge(&start, &map(json!({ "X-Api-Key": "fresh" })));
        let resealed = seal_headers(&sb, &next).unwrap();
        assert_eq!(
            open_headers(&sb, &server_with(resealed, Map::new())).unwrap(),
            map(json!({ "X-Api-Key": "fresh" }))
        );
        // Plaintext still on the row survives the replacement.
        let plain = map(json!({ "X-Org": "acme" }));
        assert_eq!(current_for_rewrite(&sb, "id", Some(&sealed), &plain), plain);
        // A readable seal is merged over, never dropped.
        let good = seal_headers(&sb, &map(json!({ "X-Keep": "k" })))
            .unwrap()
            .unwrap();
        assert_eq!(
            current_for_rewrite(&sb, "id", Some(&good), &Map::new()),
            map(json!({ "X-Keep": "k" }))
        );
    }

    #[test]
    fn the_sealing_grace_outlasts_the_old_slots_drain() {
        assert!(
            legacy_sealing_grace() > Duration::from_millis(talaria_update_layout::roll_drain_ms())
        );
    }

    #[test]
    fn header_pairs_skip_non_strings() {
        let pairs = header_pairs(&map(json!({ "A": "1", "B": 2, "C": "3" })));
        assert_eq!(
            pairs,
            vec![
                ("A".to_string(), "1".to_string()),
                ("C".to_string(), "3".to_string())
            ]
        );
    }
}
