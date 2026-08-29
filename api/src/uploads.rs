// Uploads — the read half only, for now: the canonical metadata a message
// stamps when it references files. The storage plane (bytes on disk or in a
// bucket, the upload/serve routes, the inline/download decision) is batch 5;
// what the tasks routes need is exactly resolveAttachments — validate a set
// of ids exist and return their metadata in caller order.

use sqlx::PgPool;

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
