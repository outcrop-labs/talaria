// Guided reindex — the repair path for the two ways the retrieval plane goes
// stale: the embedding model changed (TALARIA_EMBED_MODEL swap → new vector
// dimension → every index/search call fails against the old collections), or
// a collection still has the legacy dense-only (v1) schema and is missing
// hybrid keyword recall.
//
// Decisions read the LIVE Qdrant collection config, never the registry columns
// (those went stale once already: rows stamped 1024 while the collections were
// 384). The rebuild is swap-then-refill: recreate each collection in the new
// shape, purge the content-hash bookkeeping so nothing skips, then run the
// normal backfill — index-don't-copy means every point can be re-derived from
// its system of record.
import { db } from '../db/pg'
import { getSetting, setSetting } from '../audit'
import { backfillAll } from './backfill'
import { embedInfo, type EmbedInfo } from './embed'
import { collectionInfo, deleteCollection, ensureHybridCollection } from './qdrant'

export interface CollectionStatus {
  id: string
  name: string
  qdrantName: string
  pointsCount: number
  denseDim: number | null
  hybrid: boolean
  /** This collection's dense dim doesn't match the live embedding model. */
  dimMismatch: boolean
  missing: boolean
}

export interface RetrievalUpgradeStatus {
  embed: EmbedInfo | null
  collections: CollectionStatus[]
  /** Any collection whose vectors no longer match the live model — indexing
   *  and search against it are failing right now. */
  dimMismatch: boolean
  /** Any legacy dense-only collection — hybrid keyword recall available. */
  legacySchema: boolean
  needsReindex: boolean
}

let statusCache: { at: number; value: RetrievalUpgradeStatus } | null = null

/** Live upgrade status (60s cache — alerts poll this). */
export async function retrievalUpgradeStatus(force = false): Promise<RetrievalUpgradeStatus> {
  if (!force && statusCache && Date.now() - statusCache.at < 60_000) return statusCache.value
  const sql = await db()
  const embed = await embedInfo()
  const cols = (await sql`
    select id, name, qdrant_name as "qdrantName" from rag_collections order by auto desc, name asc
  `) as unknown as Array<{ id: string; name: string; qdrantName: string }>
  const collections: CollectionStatus[] = []
  for (const c of cols) {
    const info = await collectionInfo(c.qdrantName)
    collections.push({
      ...c,
      pointsCount: info?.pointsCount ?? 0,
      denseDim: info?.denseDim ?? null,
      hybrid: info?.hybrid ?? false,
      dimMismatch: !!embed && !!info?.denseDim && info.denseDim !== embed.dim,
      missing: !info,
    })
  }
  const dimMismatch = collections.some((c) => c.dimMismatch)
  const legacySchema = collections.some((c) => !c.missing && !c.hybrid)
  const value: RetrievalUpgradeStatus = {
    embed,
    collections,
    dimMismatch,
    legacySchema,
    // No embed service = nothing actionable; the existing rag-down alert covers it.
    needsReindex: !!embed && (dimMismatch || legacySchema || collections.some((c) => c.missing)),
  }
  statusCache = { at: Date.now(), value }
  return value
}

export interface ReindexStatus {
  state: 'idle' | 'running' | 'done' | 'error'
  phase?: 'rebuilding' | 'backfilling'
  startedAt?: string
  finishedAt?: string
  error?: string
}

const STATUS_KEY = 'rag_reindex_status'
export const reindexStatus = () => getSetting<ReindexStatus>(STATUS_KEY, { state: 'idle' })
const setStatus = (s: ReindexStatus) => setSetting(STATUS_KEY, s)

let reindexRunning = false

/** Rebuild every collection in the current model's shape (hybrid v2), then
 *  refill from the systems of record. Search runs thin between the swap and
 *  the backfill finishing — the admin UI says so before the button. */
export async function reindexAll(): Promise<void> {
  if (reindexRunning) return
  reindexRunning = true
  try {
    await setStatus({ state: 'running', phase: 'rebuilding', startedAt: new Date().toISOString() })
    const embed = await embedInfo()
    if (!embed) throw new Error('embedding service unreachable — bring it up, then rebuild')
    const sql = await db()
    const cols = (await sql`select id, qdrant_name as "qdrantName" from rag_collections`) as unknown as Array<{
      id: string
      qdrantName: string
    }>
    for (const c of cols) {
      await deleteCollection(c.qdrantName)
      await ensureHybridCollection(c.qdrantName, embed.dim)
      // Old point ids are gone with the collection; drop the bookkeeping so
      // content hashes can't skip the re-embed.
      await sql`delete from rag_points where collection_id = ${c.id}`
      await sql`update rag_collections set embed_dim = ${embed.dim}, schema_version = 2 where id = ${c.id}`
    }
    statusCache = null
    await setStatus({ state: 'running', phase: 'backfilling', startedAt: new Date().toISOString() })
    await backfillAll()
    await setStatus({ state: 'done', finishedAt: new Date().toISOString() })
  } catch (e) {
    await setStatus({ state: 'error', error: (e as Error).message, finishedAt: new Date().toISOString() })
  } finally {
    reindexRunning = false
    statusCache = null
  }
}
