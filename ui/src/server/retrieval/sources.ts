// Convenience indexers that target the auto collections by kind, so callers
// (channel posts, KB doc saves, …) don't resolve collection ids themselves.
// All fire-and-forget friendly — indexing must never block the write it follows.
import { db } from '../db/pg'
import { ensureAutoCollections } from './collections'
import { indexDocument, unindexDocument, type IndexDoc } from './index'

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
