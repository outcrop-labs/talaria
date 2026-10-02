// The audit trail's write side — a durable, queryable record of who changed
// what. The read side lives with the admin surfaces that show it; the write
// is what auth actions need on the way past.

use sqlx::PgPool;

/// One governance-relevant mutation.
pub struct AuditEntry<'a> {
    pub actor: &'a str,
    pub action: &'a str,
    pub target_type: &'a str,
    pub target_id: Option<&'a str>,
    pub target_label: Option<&'a str>,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
}

/// Insert one audit row. Auditing must never break the operation it records —
/// an insert failure is logged here and nothing propagates.
pub async fn log_audit(pg: &PgPool, entry: AuditEntry<'_>) {
    let result = sqlx::query(
        "insert into audit_log (actor, action, target_type, target_id, target_label, before, after) \
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(entry.actor)
    .bind(entry.action)
    .bind(entry.target_type)
    .bind(entry.target_id)
    .bind(entry.target_label)
    .bind(entry.before)
    .bind(entry.after)
    .execute(pg)
    .await;
    if let Err(e) = result {
        tracing::warn!("[audit] insert failed: {e}");
    }
}

/// Audit a mutation WITHOUT making the caller wait for it.
///
/// Every route that audits does the same three things: resolve the actor,
/// detach, and write. Three copies of that had grown by the time the
/// coding-account routes wanted a fourth, so it lives here once — the crate
/// that already owns the concept. Values are owned rather than borrowed
/// because the write outlives the request: a borrowed actor would tie this to
/// the handler's lifetime, which is exactly what spawning is for.
///
/// It takes the actor STRING, not a session user, so this crate keeps knowing
/// nothing about sessions.
pub fn spawn_audit(
    pg: &PgPool,
    actor: String,
    action: &str,
    target_type: &str,
    target_id: Option<String>,
    after: Option<serde_json::Value>,
) {
    let pg = pg.clone();
    let action = action.to_string();
    let target_type = target_type.to_string();
    tokio::spawn(async move {
        log_audit(
            &pg,
            AuditEntry {
                actor: &actor,
                action: &action,
                target_type: &target_type,
                target_id: target_id.as_deref(),
                target_label: None,
                before: None,
                after,
            },
        )
        .await;
    });
}
