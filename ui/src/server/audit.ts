// The audit trail: a durable, queryable record of who changed what, with
// before/after state. Distinct from the activity feed (which is a read model
// over app tables) — this captures governance-relevant mutations explicitly.
// Retention is configurable (app_settings.audit_retention_days); a lazy prune
// runs on reads.
import { db } from './db/pg'

export interface AuditEntry {
  actor: string
  action: string // e.g. 'role.change', 'endpoint.delete', 'agent.retire'
  targetType: string // e.g. 'user', 'endpoint', 'agent'
  targetId?: string | null
  targetLabel?: string | null
  before?: unknown
  after?: unknown
}

export interface AuditRow extends AuditEntry {
  id: string
  createdAt: string
}

const DEFAULT_RETENTION_DAYS = 90

export async function logAudit(entry: AuditEntry): Promise<void> {
  const sql = await db()
  await sql`
    insert into audit_log (actor, action, target_type, target_id, target_label, before, after)
    values (${entry.actor}, ${entry.action}, ${entry.targetType}, ${entry.targetId ?? null}, ${entry.targetLabel ?? null},
            ${entry.before === undefined ? null : sql.json(entry.before as never)},
            ${entry.after === undefined ? null : sql.json(entry.after as never)})
  `.catch(() => {}) // auditing must never break the operation it records
}

export async function getSetting<T>(key: string, fallback: T): Promise<T> {
  const sql = await db()
  const rows = (await sql`select value from app_settings where key = ${key}`) as unknown as Array<{ value: T }>
  return rows[0]?.value ?? fallback
}

export async function setSetting(key: string, value: unknown): Promise<void> {
  const sql = await db()
  await sql`
    insert into app_settings (key, value) values (${key}, ${sql.json(value as never)})
    on conflict (key) do update set value = excluded.value, updated_at = now()
  `
}

export async function auditRetentionDays(): Promise<number> {
  return getSetting('audit_retention_days', DEFAULT_RETENTION_DAYS)
}

/** Delete entries past the retention window. Cheap; called lazily on reads. */
async function prune(): Promise<void> {
  const days = await auditRetentionDays()
  if (!days || days <= 0) return // 0 = keep forever
  const sql = await db()
  await sql`delete from audit_log where created_at < now() - (${days} || ' days')::interval`.catch(() => {})
}

export async function listAudit(limit = 100): Promise<AuditRow[]> {
  void prune()
  const sql = await db()
  return (await sql`
    select id, actor, action, target_type as "targetType", target_id as "targetId",
           target_label as "targetLabel", before, after, created_at as "createdAt"
    from audit_log order by created_at desc limit ${Math.min(limit, 200)}
  `) as unknown as AuditRow[]
}
