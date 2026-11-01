// Retention of notification rows (O-15: read rows go after 90 days, unread
// after 180). An empty stub in phase 0; W2-E implements and registers the job
// (docs/notifications/delivery.md).

use sqlx::PgPool;

/// The retention pass: a STUB in phase 0 that deletes nothing.
pub async fn run_retention(_pg: &PgPool) -> Result<Option<String>, String> {
    Ok(None)
}
