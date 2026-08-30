// Version history for agent internals — port of the write half of
// ui/src/server/internal-history.ts (snapshot). The listing/recovery reads
// land with the internals routes in batch 5.
//
// A uniform snapshot store: every save appends an immutable revision, so any
// prior content is recoverable — the "versioned internals" promise applied to
// files/memory that otherwise live on disk or inside a container with no
// history of their own.

use sqlx::PgPool;

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
