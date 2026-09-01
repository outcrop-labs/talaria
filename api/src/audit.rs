// The audit trail — port of logAudit in ui/src/server/audit.ts: a durable,
// queryable record of who changed what. The read side (listAudit, retention)
// ports with the admin surfaces that show it; the write is what auth actions
// need on the way past.

use sqlx::PgPool;

/// One governance-relevant mutation (audit.ts AuditEntry).
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
/// logAudit ends in `.catch(() => {})`, so an insert failure is logged here
/// and nothing propagates.
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
