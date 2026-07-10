// Convenience indexers that target the auto collections by kind, so callers
// (channel posts, KB doc saves, and so on) don't resolve collection ids themselves.
// All fire-and-forget friendly — indexing must never block the write it follows.
import { db } from '../db/pg'
import { ensureAutoCollections, personalCollectionFor } from './collections'
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

// ── KB doc ↔ RAG sync ───────────────────────────────────────────────────────
// A doc lives in exactly one RAG collection, decided by its visibility:
//   private            → the owner's personal collection (owner + their agent only)
//   org/public + official → the org brain (grounds everyone)
//   org/public draft   → nowhere (readable in the UI, not auto-grounding)
// Every save/visibility/official change re-runs this, removing the doc from the
// collections it no longer belongs to — so nothing leaks or goes stale.
export interface KbDocSync {
  id: string
  spaceId?: string | null
  title: string
  body: string
  visibility: 'private' | 'org' | 'public'
  official: boolean
  ownerUserId: string | null
}

/** The custom collection a KB space feeds (curation: bind a space to a brain
 *  on Admin → Retrieval), if any. */
async function spaceBrain(spaceId: string | null | undefined): Promise<string | null> {
  if (!spaceId) return null
  const sql = await db()
  const rows = (await sql`
    select s.rag_collection_id as id from kb_spaces s
    join rag_collections c on c.id = s.rag_collection_id
    where s.id = ${spaceId}
  `) as unknown as Array<{ id: string }>
  return rows[0]?.id ?? null
}

export async function syncKbDoc(doc: KbDocSync): Promise<void> {
  const orgId = await autoCollectionId('org-kb')
  const personal = doc.ownerUserId ? await personalCollectionFor(doc.ownerUserId) : null
  const sql = await db()
  // Per-doc routing wins over the space default: 'auto' | 'none' | <brain id>.
  const [routing] = (await sql`select rag_routing as r from kb_docs where id = ${doc.id}`) as unknown as Array<{ r: string }>
  const route = routing?.r ?? 'auto'
  const brain =
    route !== 'auto' && route !== 'none'
      ? ((await sql`select id from rag_collections where id = ${route} and kind = 'custom'`) as unknown as Array<{ id: string }>)[0]?.id ?? null
      : route === 'auto'
        ? await spaceBrain(doc.spaceId)
        : null
  // Clear from every possible home first (idempotent), then place it. Custom
  // homes are cleared wholesale so re-routing can't leave stale copies.
  if (orgId) await unindexDocument(orgId, 'kb-doc', doc.id).catch(() => {})
  if (personal) await unindexDocument(personal.id, 'kb-doc', doc.id).catch(() => {})
  const customs = (await sql`select id from rag_collections where kind = 'custom'`) as unknown as Array<{ id: string }>
  for (const c of customs) await unindexDocument(c.id, 'kb-doc', doc.id).catch(() => {})

  if (route === 'none') return // deliberately unindexed, everywhere

  const idxDoc: IndexDoc = {
    sourceType: 'kb-doc',
    sourceId: doc.id,
    title: doc.title,
    text: `${doc.title}\n\n${doc.body}`,
    href: `/knowledge/${doc.id}`,
  }
  if (doc.visibility === 'private') {
    // Privacy trumps routing: a private doc only ever reaches its owner's
    // personal brain, whatever the routing says.
    if (personal) await indexDocument(personal.id, idxDoc).catch(() => {})
  } else if (brain) {
    // Explicit assignment (doc- or space-level) — access governed by the
    // brain's own bindings.
    await indexDocument(brain, idxDoc).catch(() => {})
  } else if (doc.official && orgId) {
    await indexDocument(orgId, idxDoc).catch(() => {})
  }
}

/** Remove a doc from every KB collection it might be in (on delete). */
export async function unindexKbDoc(id: string, ownerUserId: string | null): Promise<void> {
  const orgId = await autoCollectionId('org-kb')
  if (orgId) await unindexDocument(orgId, 'kb-doc', id).catch(() => {})
  if (ownerUserId) {
    const personal = await personalCollectionFor(ownerUserId)
    if (personal) await unindexDocument(personal.id, 'kb-doc', id).catch(() => {})
  }
  const sql = await db()
  const customs = (await sql`select id from rag_collections where kind = 'custom'`) as unknown as Array<{ id: string }>
  for (const c of customs) await unindexDocument(c.id, 'kb-doc', id).catch(() => {})
}

/** Re-route every doc in a space through syncKbDoc — run after (re)binding a
 *  space to a brain, so existing docs move immediately. */
export async function resyncSpaceDocs(spaceId: string): Promise<number> {
  const sql = await db()
  const docs = (await sql`
    select d.id, d.space_id as "spaceId", d.title, d.body, d.visibility, d.official, d.owner_user_id as "ownerUserId"
    from kb_docs d where d.space_id = ${spaceId}
  `) as unknown as KbDocSync[]
  for (const d of docs) await syncKbDoc(d).catch(() => {})
  return docs.length
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
