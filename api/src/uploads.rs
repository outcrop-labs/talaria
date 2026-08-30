// Uploads — the read half only, for now: the canonical metadata a message
// stamps when it references files, and (since the storage read plane crossed
// with the plan-draft slice) the BYTES a transcript rebuild re-reads from a
// textual attachment. The write/serve routes, the inline/download decision
// and the storage admin stay batch 5; what has crossed is exactly what
// readers need — resolveAttachments for metadata, getUpload for bytes.

use sqlx::PgPool;
use std::sync::LazyLock;

use crate::secretbox::SecretBox;
use crate::storage::read_blob;

/// An upload's canonical metadata (uploads.ts Attachment). `refType` is set
/// only on knowledge/artifact reference chips (refs.rs); upload rows never
/// carry it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub id: String,
    pub filename: String,
    pub mime: String,
    pub size: i64,
}

/// Validate that a set of attachment ids exist (before stamping them on a
/// message) and return their canonical metadata, preserving caller order —
/// including duplicates, which the caller's list asked for twice. Unknown ids
/// are dropped silently: a missing row is not worth failing a whole patch
/// over, and the TS behavior is the same.
pub async fn resolve_attachments(
    pg: &PgPool,
    ids: &[String],
) -> Result<Vec<Attachment>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
        "select id::text, filename, mime, size from uploads where id = any($1::uuid[])",
    )
    .bind(ids.to_vec())
    .fetch_all(pg)
    .await?;
    let by_id: std::collections::HashMap<String, Attachment> = rows
        .into_iter()
        .map(|(id, filename, mime, size)| {
            (
                id.clone(),
                Attachment {
                    id,
                    filename,
                    mime,
                    size,
                },
            )
        })
        .collect();
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

// ── The byte read ────────────────────────────────────────────────────────────

/// One upload's bytes, wherever the row's `path` says they live (getUpload):
/// the row's metadata plus readBlob — disk, the configured bucket, or the
/// replica behind it. A blob that cannot be found is None; the callers that
/// shape transcripts treat a missing file as context that quietly isn't
/// there, never as a failed read. `(bytes, mime, filename)`.
pub async fn get_upload(
    pg: &PgPool,
    sb: &SecretBox,
    id: &str,
) -> Result<Option<(Vec<u8>, String, String)>, sqlx::Error> {
    let row: Option<(String, String, String)> =
        sqlx::query_as("select filename, mime, path from uploads where id = $1::uuid")
            .bind(id)
            .fetch_optional(pg)
            .await?;
    let Some((filename, mime, path)) = row else {
        return Ok(None);
    };
    // The blob read is not a database concern: a lost blob (file deleted,
    // bucket rehomed) is a missing upload, not a failed query.
    Ok(read_blob(pg, sb, &path)
        .await
        .map(|bytes| (bytes, mime, filename)))
}

/// uploads.ts TEXT_MIME — the mimes whose bytes become prompt context.
static TEXT_MIME: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"^text/|^application/(json|xml|javascript|x-yaml|yaml|csv|toml|sql|markdown)|(\+json|\+xml)$",
    )
    .unwrap()
});

/// How much of a file's text rides in one block (FILE_CLIP): a transcript is
/// context, not an archive.
const FILE_CLIP: usize = 6_000;

fn file_candidates(attachments: &serde_json::Value) -> Vec<&serde_json::Value> {
    let Some(arr) = attachments.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .filter(|a| {
            a.as_object().is_some_and(|o| !o.contains_key("refType"))
                && a.get("mime")
                    .and_then(|m| m.as_str())
                    .is_some_and(|m| TEXT_MIME.is_match(m))
        })
        .collect()
}

/// The prompt block a message's textual file uploads contribute
/// (attachmentTextBlocks): up to `max_files` text-mime attachments, each
/// re-read from storage and clipped. Ref chips are NOT file uploads (they
/// carry their content inline — ref_blocks); a non-array or a row whose blob
/// is gone contributes nothing, exactly TS's `.catch(() => null)` skip.
pub async fn attachment_text_blocks(
    pg: &PgPool,
    sb: &SecretBox,
    attachments: &serde_json::Value,
    max_files: usize,
) -> String {
    let files = file_candidates(attachments);
    let mut blocks: Vec<String> = Vec::new();
    for f in files.iter().take(max_files) {
        let id = f.get("id").and_then(|i| i.as_str()).unwrap_or("");
        let Ok(Some((bytes, _mime, filename))) = get_upload(pg, sb, id).await else {
            continue;
        };
        // Buffer.toString('utf8') replaces invalid sequences with U+FFFD —
        // from_utf8_lossy is the same replacement, not a failure.
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let clipped = if crate::body::utf16_len(&text) > FILE_CLIP {
            format!(
                "{}\n[clipped]",
                crate::body::truncate_utf16(&text, FILE_CLIP)
            )
        } else {
            text
        };
        blocks.push(format!(
            "\n\n--- Attached file: \"{filename}\" ---\n{clipped}"
        ));
    }
    blocks.join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_mime_matches_the_js_regex() {
        for m in [
            "text/plain",
            "text/csv",
            "application/json",
            "application/xml",
            "application/javascript",
            "application/x-yaml",
            "application/yaml",
            "application/csv",
            "application/toml",
            "application/sql",
            "application/markdown",
            "application/vnd.api+json",
            "application/ld+json",
            "image/svg+xml",
            "application/xhtml+xml",
            // The ^-anchored alternatives have no end anchor in the JS
            // pattern, so a PREFIX matches: "jsonl" starts with "json".
            "application/jsonl",
        ] {
            assert!(TEXT_MIME.is_match(m), "should match {m}");
        }
        for m in [
            "image/png",
            "application/pdf",
            "application/octet-stream",
            "textish/plain",
            "application/fooxml",
        ] {
            assert!(!TEXT_MIME.is_match(m), "should not match {m}");
        }
    }

    /// The FILTER is the whole testable surface without storage: which array
    /// members are file candidates at all. The bytes come from read_blob,
    /// which storage's own suite covers.
    #[test]
    fn file_candidates_exclude_refs_nonobjects_and_nontext() {
        let a = json!([
            { "id": "1", "mime": "text/plain", "filename": "a.txt" },
            { "id": "2", "mime": "image/png" },
            { "refType": "kb-doc", "mime": "text/plain", "content": "x" },
            { "mime": "text/plain" },
            "not an object",
            { "id": "3", "mime": "application/json" }
        ]);
        let c = file_candidates(&a);
        // Three candidates: the id-less text entry DOES pass the filter — in
        // TS its read then misses (getUpload(undefined) → catch → skip), so
        // the filter itself must keep it.
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].get("id").unwrap(), "1");
        assert!(c[1].get("id").is_none());
        assert_eq!(c[2].get("id").unwrap(), "3");
        // A non-array answers no candidates at all (refBlocks' shape rule).
        assert!(file_candidates(&json!(null)).is_empty());
        assert!(file_candidates(&json!({})).is_empty());
    }
}
