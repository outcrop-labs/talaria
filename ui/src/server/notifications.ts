// Notifications — a user's inbox. Mentions today; more kinds later.
import { db } from './db/pg'

export interface Notification {
  id: string
  kind: string
  title: string
  body: string
  href: string
  readAt: string | null
  createdAt: string
}

export async function addNotification(
  userId: string,
  n: { kind: string; title: string; body?: string; href?: string },
): Promise<void> {
  const sql = await db()
  await sql`
    insert into notifications (user_id, kind, title, body, href)
    values (${userId}, ${n.kind}, ${n.title}, ${n.body ?? ''}, ${n.href ?? ''})
  `
}

export async function listNotifications(userId: string, limit = 50): Promise<Notification[]> {
  const sql = await db()
  return (await sql`
    select id, kind, title, body, href, read_at as "readAt", created_at as "createdAt"
    from notifications where user_id = ${userId}
    order by created_at desc limit ${limit}
  `) as unknown as Notification[]
}

export async function unreadCount(userId: string): Promise<number> {
  const sql = await db()
  const rows = await sql`select count(*)::int as n from notifications where user_id = ${userId} and read_at is null`
  return (rows[0] as { n: number }).n
}

/** Mark specific notifications read, or all of the user's when ids is omitted. */
export async function markNotificationsRead(userId: string, ids?: string[]): Promise<void> {
  const sql = await db()
  if (ids && ids.length > 0) {
    await sql`update notifications set read_at = now() where user_id = ${userId} and id = any(${ids}) and read_at is null`
  } else {
    await sql`update notifications set read_at = now() where user_id = ${userId} and read_at is null`
  }
}
