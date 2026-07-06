// Convenience indexers that target the auto collections by kind, so callers
// (channel posts, KB doc saves, …) don't resolve collection ids themselves.
// All fire-and-forget friendly — indexing must never block the write it follows.
import { db } from '../db/pg'
import { ensureAutoCollections } from './collections'
import { indexDocument, unindexDocument, type IndexDoc } from './index'
import { deleteByFilter } from './qdrant'

async function autoCollectionId(kind: 'activity' | 'org-kb'): Promise<string | null> {
  const sql = await db()
  const rows = (await sql`select id from rag_collections where kind = ${kind} and auto limit 1`) as unknown as Array<{ id: string }>
  if (rows[0]) return rows[0].id
  await ensureAutoCollections().catch(() => {})
  const again = (await sql`select id from rag_collections where kind = ${kind} and auto limit 1`) as unknown as Array<{ id: string }>
  return again[0]?.id ?? null
}

export async function indexActivity(doc: IndexDoc): Promise<void> {
  const id = await autoCollectionId('activity')
  if (id) await indexDocument(id, doc).catch(() => {})
}

export async function indexOrgKb(doc: IndexDoc): Promise<void> {
  const id = await autoCollectionId('org-kb')
  if (id) await indexDocument(id, doc).catch(() => {})
}

export async function unindexOrgKb(sourceType: string, sourceId: string): Promise<void> {
  const id = await autoCollectionId('org-kb')
  if (id) await unindexDocument(id, sourceType, sourceId).catch(() => {})
}

export async function unindexActivity(sourceType: string, sourceId: string): Promise<void> {
  const id = await autoCollectionId('activity')
  if (id) await unindexDocument(id, sourceType, sourceId).catch(() => {})
}

// ── Domain shapers ──────────────────────────────────────────────────────────
// Tickets and comments are board-scoped (payload.boardId), so activityScope's
// board filter already governs who can retrieve them.
export async function indexTicket(t: { id: string; boardId: string; ticketRef?: string | null; title: string; description?: string | null }): Promise<void> {
  const text = [t.title, t.description ?? ''].filter((s) => s.trim()).join('\n\n')
  await indexActivity({
    sourceType: 'ticket',
    sourceId: t.id,
    title: `${t.ticketRef ?? 'Ticket'}: ${t.title}`,
    text,
    payload: { boardId: t.boardId },
    href: `/boards/${t.boardId}/${t.id}`,
  })
}

export async function indexTicketComment(c: { id: string; taskId: string; boardId: string; ticketRef?: string | null; author: string; content: string }): Promise<void> {
  await indexActivity({
    sourceType: 'comment',
    sourceId: c.id,
    title: `${c.ticketRef ?? 'Ticket'} · ${c.author}`,
    text: c.content,
    payload: { boardId: c.boardId },
    href: `/boards/${c.boardId}/${c.taskId}`,
  })
}

/** Purge all activity points for a container (e.g. a deleted channel or board)
 *  by payload field, without enumerating each item. Leaves the rag_points
 *  bookkeeping rows — they're harmless dead refs (UUID source ids never recur)
 *  and search reads only from Qdrant. */
export async function purgeActivityByField(field: 'channelId' | 'boardId', value: string): Promise<void> {
  const sql = await db()
  const rows = (await sql`select qdrant_name as "qdrantName" from rag_collections where kind = 'activity' and auto limit 1`) as unknown as Array<{ qdrantName: string }>
  const name = rows[0]?.qdrantName
  if (name) await deleteByFilter(name, { must: [{ key: field, match: { value } }] }).catch(() => {})
}
