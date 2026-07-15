// The retrieval core: index a source document into a collection (chunk → embed
// → upsert, idempotent by content hash), and search across a principal's
// accessible collections with ranked, merged results. Index-don't-copy — points
// carry a pointer (sourceType/sourceId + snippet + ACL), never the system of
// record; callers fetch the live record from that pointer.
import { createHash, randomUUID } from 'node:crypto'
import { db } from '../db/pg'
import { embed, embedOne } from './embed'
import { upsertPoints, deletePoints, search as qdrantSearch, hybridQuery, type QdrantPoint } from './qdrant'
import { collectionsForPrincipal, getCollection, type RagCollection } from './collections'
import { getRerankConfig, rerank } from './rerank'
import { sparseEncode } from './sparse'

export interface IndexDoc {
  sourceType: string // 'kb-doc' | 'channel' | 'chat' | 'plan' | 'research' | 'ticket' | 
  sourceId: string
  title?: string
  text: string
  /** Extra payload stored on every chunk (e.g. { boardId, channelId } for ACL). */
  payload?: Record<string, unknown>
  href?: string
}

const hash = (s: string) => createHash('sha256').update(s).digest('hex')

/** Paragraph-aware chunking with a soft size cap (~500 tokens ≈ 2000 chars). */
function chunk(text: string, max = 2000): string[] {
  const paras = text.split(/\n\s*\n/).map((p) => p.trim()).filter(Boolean)
  const out: string[] = []
  let cur = ''
  for (const p of paras) {
    if (cur && cur.length + p.length > max) {
      out.push(cur)
      cur = ''
    }
    // A single oversized paragraph gets hard-split.
    if (p.length > max) {
      if (cur) { out.push(cur); cur = '' }
      for (let i = 0; i < p.length; i += max) out.push(p.slice(i, i + max))
    } else {
      cur = cur ? `${cur}\n\n${p}` : p
    }
  }
  if (cur) out.push(cur)
  return out.slice(0, 100) // safety cap
}

/** Index (or re-index) a document into a collection. No-op when unchanged. */
export async function indexDocument(collectionId: string, doc: IndexDoc): Promise<{ chunks: number; skipped: boolean }> {
  const col = await getCollection(collectionId)
  if (!col) throw new Error('unknown collection')
  const sql = await db()
  const h = hash(doc.text + (doc.title ?? ''))

  const [prev] = (await sql`
    select point_ids as "pointIds", content_hash as "contentHash"
    from rag_points where collection_id = ${collectionId} and source_type = ${doc.sourceType} and source_id = ${doc.sourceId}
  `) as unknown as Array<{ pointIds: string[]; contentHash: string }>
  if (prev && prev.contentHash === h) return { chunks: prev.pointIds.length, skipped: true }

  const chunks = chunk(doc.text)
  if (chunks.length === 0) {
    // Empty now — drop any prior points.
    if (prev) {
      await deletePoints(col.qdrantName, prev.pointIds)
      await sql`delete from rag_points where collection_id = ${collectionId} and source_type = ${doc.sourceType} and source_id = ${doc.sourceId}`
    }
    return { chunks: 0, skipped: false }
  }

  const vectors = await embed(chunks)
  const hybrid = col.schemaVersion >= 2
  const points: QdrantPoint[] = chunks.map((c, i) => ({
    id: randomUUID(),
    vector: vectors[i]!,
    // Hybrid collections also index the chunk's terms (title included, so an
    // exact-name query lands even when the body never repeats it).
    ...(hybrid ? { sparse: sparseEncode(`${doc.title ?? ''}\n${c}`) } : {}),
    payload: {
      ...(doc.payload ?? {}),
      sourceType: doc.sourceType,
      sourceId: doc.sourceId,
      title: doc.title ?? null,
      snippet: c.slice(0, 400),
      href: doc.href ?? null,
      chunk: i,
    },
  }))
  await upsertPoints(col.qdrantName, points, hybrid)
  if (prev) await deletePoints(col.qdrantName, prev.pointIds)

  const ids = points.map((p) => p.id)
  await sql`
    insert into rag_points (collection_id, source_type, source_id, point_ids, content_hash)
    values (${collectionId}, ${doc.sourceType}, ${doc.sourceId}, ${sql.json(ids)}, ${h})
    on conflict (collection_id, source_type, source_id)
    do update set point_ids = ${sql.json(ids)}, content_hash = ${h}, updated_at = now()
  `
  return { chunks: ids.length, skipped: false }
}

/** Remove a source doc from a collection. */
export async function unindexDocument(collectionId: string, sourceType: string, sourceId: string): Promise<void> {
  const col = await getCollection(collectionId)
  if (!col) return
  const sql = await db()
  const [row] = (await sql`
    select point_ids as "pointIds" from rag_points
    where collection_id = ${collectionId} and source_type = ${sourceType} and source_id = ${sourceId}
  `) as unknown as Array<{ pointIds: string[] }>
  if (row) {
    await deletePoints(col.qdrantName, row.pointIds)
    await sql`delete from rag_points where collection_id = ${collectionId} and source_type = ${sourceType} and source_id = ${sourceId}`
  }
}

export interface RetrievalHit {
  collection: string
  score: number
  sourceType: string
  sourceId: string
  title: string | null
  snippet: string
  href: string | null
}

/** The channel + board ids a principal may see — used to ACL-filter the ambient
 *  activity index at the item level (a user must not retrieve a channel they're
 *  not in). Agents are scoped to the boards their policy allows. */
async function activityScope(principal: { userId?: string; agentModel?: string }): Promise<Record<string, unknown> | null> {
  const sql = await db()
  if (principal.userId) {
    const chans = (await sql`select channel_id as id from channel_members where user_id = ${principal.userId}`) as unknown as Array<{ id: string }>
    const boards = (await sql`
      select b.id from boards b
      left join board_members m on m.board_id = b.id and m.user_id = ${principal.userId}
      left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${principal.userId}
      where m.user_id is not null or tm.user_id is not null
    `) as unknown as Array<{ id: string }>
    return {
      should: [
        { key: 'channelId', match: { any: chans.map((c) => c.id) } },
        { key: 'boardId', match: { any: boards.map((b) => b.id) } },
        // Plans (conversations + their living documents) are private to their
        // owner — only that user retrieves them from the ambient index.
        { key: 'planOwnerId', match: { value: principal.userId } },
        // Distilled agent chats are likewise owner-private.
        { key: 'ownerUserId', match: { value: principal.userId } },
      ],
    }
  }
  if (principal.agentModel) {
    // Agents: boards whose policy allows them (channels an agent is in are also
    // fine, but boards are the primary agent scope).
    const boards = (await sql`
      select b.id from boards b
      where b.allow_all_agents
         or exists (select 1 from board_agents ba where ba.board_id = b.id and ba.agent_model = ${principal.agentModel})
    `) as unknown as Array<{ id: string }>
    return { should: [{ key: 'boardId', match: { any: boards.map((b) => b.id) } }] }
  }
  return { should: [] } // no scope → nothing from activity
}

/** Search across the principal's accessible collections. Recall per collection
 *  (HYBRID dense+keyword RRF on v2 collections — exact identifiers like env
 *  vars, ticket numbers, and error strings rank alongside semantic matches;
 *  dense-only on legacy v1) → merge + dedupe → (when configured) cross-encoder
 *  RERANK over the merged candidate pool → top `limit`. Neither cosine nor RRF
 *  scores are comparable across collections, so the reranker is what makes the
 *  merged ranking honest; without one we keep the per-collection score order. */
export async function searchForPrincipal(
  principal: { userId?: string; agentModel?: string },
  query: string,
  opts: { limit?: number; collectionIds?: string[] } = {},
): Promise<RetrievalHit[]> {
  let cols = await collectionsForPrincipal(principal)
  if (opts.collectionIds?.length) cols = cols.filter((c) => opts.collectionIds!.includes(c.id))
  if (cols.length === 0) return []
  const limit = opts.limit ?? 8
  const rerankCfg = await getRerankConfig()
  const reranking = rerankCfg.provider !== 'off'
  // With a reranker, over-fetch per collection so it has a real pool to work.
  const perColLimit = reranking ? Math.max(limit, Math.ceil((rerankCfg.candidates ?? 30) / cols.length) + 2) : limit
  const vec = await embedOne(query)
  const sparse = sparseEncode(query)
  const activityFilter = await activityScope(principal)

  const perCol = await Promise.all(
    cols.map(async (c: RagCollection) => {
      // Ambient activity is ACL-filtered per item; curated/custom collections
      // are gated at the collection level (their binding).
      const filter = c.kind === 'activity' ? activityFilter ?? undefined : undefined
      const hits =
        c.schemaVersion >= 2
          ? await hybridQuery(c.qdrantName, vec, sparse, perColLimit, filter).catch(() => [])
          : await qdrantSearch(c.qdrantName, vec, perColLimit, filter).catch(() => [])
      return hits.map((h) => ({
        collection: c.name,
        score: h.score,
        sourceType: String(h.payload.sourceType ?? ''),
        sourceId: String(h.payload.sourceId ?? ''),
        title: (h.payload.title as string | null) ?? null,
        snippet: String(h.payload.snippet ?? ''),
        href: (h.payload.href as string | null) ?? null,
      }))
    }),
  )
  // Merge, dedupe by source, rank by vector score.
  const seen = new Set<string>()
  const merged = perCol
    .flat()
    .sort((a, b) => b.score - a.score)
    .filter((h) => {
      const k = `${h.sourceType}:${h.sourceId}`
      if (seen.has(k)) return false
      seen.add(k)
      return true
    })
  if (!reranking || merged.length <= 1) return merged.slice(0, limit)

  // Precision pass: rescore the pool with the configured cross-encoder.
  const pool = merged.slice(0, rerankCfg.candidates ?? 30)
  const scores = await rerank(query, pool.map((h) => `${h.title ?? ''}\n${h.snippet}`.trim()))
  if (!scores) return merged.slice(0, limit) // provider hiccup → vector order
  return pool
    .map((h, i) => ({ ...h, score: scores[i]! }))
    .sort((a, b) => b.score - a.score)
    .slice(0, limit)
}
