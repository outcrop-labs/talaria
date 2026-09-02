// Version history for agent internals: the write half (snapshot) plus the
// listing/recovery reads /api/history serves.
//
// A uniform snapshot store: every save appends an immutable revision, so any
// prior content is recoverable — the "versioned internals" promise applied to
// files/memory that otherwise live on disk or inside a container with no
// history of their own.

use sqlx::PgPool;

use crate::agent_auth::epoch_ms_to_iso;

/// One revision summary. Key order is the wire contract — the select order
/// below, not alphabetical.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub id: String,
    pub created_by: Option<String>,
    pub created_at: String,
    /// Character length — cheap preview without shipping every full body.
    pub size: i32,
}

/// Append a revision — but only when the content actually changed from the
/// latest one (idempotent saves don't spam history).
pub async fn snapshot(
    pg: &PgPool,
    kind: &str,
    owner_key: &str,
    content: &str,
    author: Option<&str>,
) -> Result<(), sqlx::Error> {
    let latest: Option<(String,)> = sqlx::query_as(
        "select content from internal_versions \
         where kind = $1 and owner_key = $2 \
         order by created_at desc limit 1",
    )
    .bind(kind)
    .bind(owner_key)
    .fetch_optional(pg)
    .await?;
    if latest.is_some_and(|(content_,)| content_ == content) {
        return Ok(());
    }
    sqlx::query(
        "insert into internal_versions (kind, owner_key, content, created_by) values ($1, $2, $3, $4)",
    )
    .bind(kind)
    .bind(owner_key)
    .bind(content)
    .bind(author)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn list_history(
    pg: &PgPool,
    kind: &str,
    owner_key: &str,
) -> Result<Vec<Revision>, sqlx::Error> {
    // `size` is PG length(content) — characters, not bytes — and createdAt
    // rides the epoch-ms → ISO path.
    let rows: Vec<(String, Option<String>, i64, i32)> = sqlx::query_as(
        "select id::text, created_by, (trunc(extract(epoch from created_at) * 1000))::bigint, \
             length(content) \
         from internal_versions \
         where kind = $1 and owner_key = $2 \
         order by created_at desc limit 50",
    )
    .bind(kind)
    .bind(owner_key)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, created_by, created_ms, size)| Revision {
            id,
            created_by,
            created_at: epoch_ms_to_iso(created_ms),
            size,
        })
        .collect())
}

pub async fn get_revision(
    pg: &PgPool,
    kind: &str,
    owner_key: &str,
    id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "select content from internal_versions \
         where id = $1::uuid and kind = $2 and owner_key = $3",
    )
    .bind(id)
    .bind(kind)
    .bind(owner_key)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|(content,)| content))
}
