// The retrieval core: index a source document into a collection (chunk → embed
// → upsert, idempotent by content hash), and search across a principal's
// accessible collections with ranked, merged results.
//
// What a point actually holds — be precise about this, the ACL depends on it:
// a pointer (sourceType/sourceId/href/title), an ACL payload, and a VERBATIM
// snippet: the first 400 chars of the chunk. Search RETURNS that snippet and
// callers do not re-fetch the system of record, so for a channel message, a
// ticket, or a comment those 400 chars ARE the content. Retrieval is a read of
// the content itself, never a pointer a later permission check re-gates.
//
// Two gates therefore stand between a principal and that text, and nothing
// else stands behind them:
//   1. the collection's bindings          (collectionsForPrincipal)
//   2. the per-item payload filter        (activityScope / docScope below)
// Both run on every collection kind. Every point written anywhere must carry
// the ACL payload its kind's filter reads (container ids for activity, DocAcl
// everywhere else): a point without one simply never matches — fail closed, so
// an un-ACL'd write is invisible rather than public. The payload is part of the
// content hash, so a visibility change re-indexes the point rather than leaving
// a stale ACL behind.
import { createHash, randomUUID } from 'node:crypto'
import { db } from '../db/pg'
import { agentBoardPolicySql, boardVisibilitySql } from '../boards'
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
  /** Extra payload stored on every chunk. This is the ACL — the activity index
   *  reads container ids ({ channelId, boardId, planOwnerId, ownerUserId,
   *  orgWide }); every other kind reads { visibility, ownerUserId } (see
   *  DocAcl). A point whose payload carries neither is unreachable. */
  payload?: Record<string, unknown>
  href?: string
}

/** The item ACL carried by kb-doc / artifact / research points — i.e. anything
 *  landing in an org-kb, custom, or personal collection. `visibility` is the
 *  EFFECTIVE visibility (a doc inheriting from a private space is private),
 *  never the raw column. */
export interface DocAcl {
  visibility: 'private' | 'org' | 'public'
  ownerUserId: string | null
  spaceId?: string | null
}

/** Bump when the ACL payload shape changes: it feeds the content hash, so
 *  existing points re-index (and pick the new payload up) on the next
 *  backfill/sweep instead of lingering with a stale or absent ACL. */
const ACL_SCHEMA = 2

const hash = (s: string) => createHash('sha256').update(s).digest('hex')

/** Order-independent payload digest — the payload is part of a point's
 *  identity (it's the ACL), so a visibility change must invalidate the hash
 *  even when the text is untouched. */
const payloadDigest = (p?: Record<string, unknown>): string =>
  JSON.stringify(
    Object.entries(p ?? {})
      .filter(([, v]) => v !== undefined)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
  )

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
  const h = hash([doc.text, doc.title ?? '', ACL_SCHEMA, payloadDigest(doc.payload)].join('\u0000'))

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
      where ${boardVisibilitySql(sql, principal.userId)}
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
        // Content explicitly published org-wide (e.g. org research reports).
        { key: 'orgWide', match: { value: true } },
      ],
    }
  }
  if (principal.agentModel) {
    // A personal assistant retrieves as its OWNER'S PROXY: the owner's
    // channels, boards, plans, and distilled history — exactly what the owner
    // themselves could retrieve, nothing more.
    const [pa] = (await sql`
      select owner_user_id as owner from agent_defs
      where model = ${principal.agentModel} and owner_user_id is not null
    `) as unknown as Array<{ owner: string }>
    if (pa?.owner) return activityScope({ userId: pa.owner })
    // Org agents: boards whose policy allows them (channels an agent is in are
    // also fine, but boards are the primary agent scope) + org-wide content.
    const boards = (await sql`
      select b.id from boards b
      where ${agentBoardPolicySql(sql, principal.agentModel)}
    `) as unknown as Array<{ id: string }>
    return {
      should: [
        { key: 'boardId', match: { any: boards.map((b) => b.id) } },
        { key: 'orgWide', match: { value: true } },
      ],
    }
  }
  return { should: [] } // no scope → nothing from activity
}

/** The human a principal retrieves AS: themselves, or — for a personal
 *  assistant — its owner. Org agents have no human identity. */
async function effectiveUserId(principal: { userId?: string; agentModel?: string }): Promise<string | null> {
  if (principal.userId) return principal.userId
  if (!principal.agentModel) return null
  const sql = await db()
  const [pa] = (await sql`
    select owner_user_id as owner from agent_defs
    where model = ${principal.agentModel} and owner_user_id is not null
  `) as unknown as Array<{ owner: string }>
  return pa?.owner ?? null
}

/** Item-level ACL for the DOCUMENT collections (org-kb, custom, personal) — the
 *  counterpart to activityScope. Collection bindings say which brains you may
 *  read; this says which items inside them you may read, because a brain is not
 *  uniform: a custom brain fed by a KB space holds docs of mixed visibility,
 *  and a private doc that landed there (or in the org brain via a backfill bug)
 *  must not come back for anyone but its owner.
 *
 *  Mirrors kb-perms.canRead: org/public → any resolved member; private → owner
 *  (a personal assistant reading as its owner). Points with no `visibility` in
 *  their payload match nothing — they re-acquire one on the next backfill. */
async function docScope(principal: { userId?: string; agentModel?: string }): Promise<Record<string, unknown>> {
  const uid = await effectiveUserId(principal)
  const should: Array<Record<string, unknown>> = [{ key: 'visibility', match: { any: ['org', 'public'] } }]
  if (uid) should.push({ must: [{ key: 'visibility', match: { value: 'private' } }, { key: 'ownerUserId', match: { value: uid } }] })
  return { should }
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
  const [activityFilter, documentFilter] = await Promise.all([activityScope(principal), docScope(principal)])

  const perCol = await Promise.all(
    cols.map(async (c: RagCollection) => {
      // EVERY kind is ACL-filtered per item, not just activity: the collection
      // binding is the outer gate, this is the inner one.
      const filter = c.kind === 'activity' ? activityFilter ?? undefined : documentFilter
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
