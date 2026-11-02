// The ack (I-18) and, from W2-E, the deferred push.
//
// A client that SURFACED a row to the person — toasted it to someone looking,
// or posted an OS banner — acks it, and `surfaced_at` records the first time.
// W2-E's deferred push fires only for rows that are still unread and
// un-acked when their `push_due_at` comes (docs/notifications/delivery.md).

use sqlx::PgPool;

/// Record that the caller's client surfaced these rows. Only the caller's own
/// rows are touched (another person's id acks nothing), and the first ack
/// wins. Answers how many rows were the caller's.
pub async fn ack(pg: &PgPool, user_id: &str, ids: &[String]) -> Result<u64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let res = sqlx::query(
        "update notifications set surfaced_at = coalesce(surfaced_at, now()) \
         where user_id = $1::uuid and id = any($2::uuid[])",
    )
    .bind(user_id)
    .bind(ids)
    .execute(pg)
    .await?;
    Ok(res.rows_affected())
}

/// The deferred-push pass: a STUB in phase 0 (nothing sets `push_due_at`
/// yet). W2-E implements it and registers its job.
pub async fn run_deferred_push(_pg: &PgPool) -> Result<Option<String>, String> {
    Ok(None)
}
