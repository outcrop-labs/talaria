// Version history for agent internals (skills, memory). A uniform snapshot
// store: every save appends an immutable revision, so any prior content is
// recoverable — the "versioned internals" promise applied to files/memory that
// otherwise live on disk or inside a container with no history of their own.
import { db } from './db/pg'

export type InternalKind = 'skill' | 'memory' | 'kb-doc' | 'kb-space' | 'artifact' | 'template'

export interface Revision {
  id: string
  createdBy: string | null
  createdAt: string
  /** Character length — cheap preview without shipping every full body. */
  size: number
}

/** Append a revision — but only when the content actually changed from the
 *  latest one (idempotent saves don't spam history). */
export async function snapshot(kind: InternalKind, ownerKey: string, content: string, author: string | null): Promise<void> {
  const sql = await db()
  const [latest] = (await sql`
    select content from internal_versions
    where kind = ${kind} and owner_key = ${ownerKey}
    order by created_at desc limit 1
  `) as unknown as Array<{ content: string }>
  if (latest && latest.content === content) return
  await sql`
    insert into internal_versions (kind, owner_key, content, created_by)
    values (${kind}, ${ownerKey}, ${content}, ${author})
  `
}

export async function listHistory(kind: InternalKind, ownerKey: string): Promise<Revision[]> {
  const sql = await db()
  return (await sql`
    select id, created_by as "createdBy", created_at as "createdAt", length(content) as size
    from internal_versions
    where kind = ${kind} and owner_key = ${ownerKey}
    order by created_at desc limit 50
  `) as unknown as Revision[]
}

export async function getRevision(kind: InternalKind, ownerKey: string, id: string): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`
    select content from internal_versions
    where id = ${id} and kind = ${kind} and owner_key = ${ownerKey}
  `) as unknown as Array<{ content: string }>
  return rows[0]?.content ?? null
}
