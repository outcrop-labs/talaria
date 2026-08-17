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
//
// THE REBUILD IS A DURABLE RUN NOW, and this was the worst place in the product
// not to be one: `let reindexRunning = false` plus a status blob, fired as a
// bare `void reindexAll()`, around the single most destructive sequence Talaria
// performs — DROP a collection, purge its bookkeeping, refill. A deploy landing
// in the middle left the blob saying state:'running' forever with the index half
// rebuilt and nothing coming back for it. The steps, the checkpoint and the
// re-entry rules live in server/runs/defs/reindex.ts; what stays here is the
// live upgrade status, the public verb, and the READ SHAPE the admin panel has
// always consumed — now a projection of the run row.
import { db } from '../db/pg'
import { embedInfo, type EmbedInfo } from './embed'
import { collectionInfo } from './qdrant'
import { REINDEX_KIND, startReindex, type ReindexCheckpoint } from '../runs/defs/reindex'
import { latestRunOfKind, type KindRunView } from '../runs/store'

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

/** THE READ SHAPE, unchanged: `components/admin/retrieval.ts` declares exactly
 *  these fields and polls while `state === 'running'`. */
export interface ReindexStatus {
  state: 'idle' | 'running' | 'done' | 'error'
  phase?: 'rebuilding' | 'backfilling'
  startedAt?: string
  finishedAt?: string
  error?: string
}

/** Project the run row onto the four states the panel knows. Same rules as the
 *  backfill's projection next door and for the same reasons: `queued` is
 *  RUNNING (a reclaim or a `retry` will move it within the minute, and the
 *  reason is in `phase`), `cancelled` is DONE with the reason attached. */
function projectStatus(run: KindRunView | null): ReindexStatus {
  if (!run) return { state: 'idle' }
  const cp = run.checkpoint as ReindexCheckpoint | null
  const at = { ...(run.startedAt ? { startedAt: run.startedAt } : {}), ...(run.finishedAt ? { finishedAt: run.finishedAt } : {}) }
  // The two words the panel prints come straight off the checkpoint, which is
  // the run's own statement of which half it is in. Before the first checkpoint
  // there is no half yet, and 'rebuilding' is what it is about to do.
  const phase = { phase: cp?.phase ?? ('rebuilding' as const) }
  if (run.state === 'error') return { state: 'error', error: run.error ?? 'the rebuild failed', ...at }
  if (run.state === 'cancelled') return { state: 'done', ...(run.error ? { error: run.error } : {}), ...at }
  if (run.state === 'done') return { state: 'done', ...at }
  return { state: 'running', ...phase, ...at }
}

export const reindexStatus = async (): Promise<ReindexStatus> => projectStatus(await latestRunOfKind(REINDEX_KIND).catch(() => null))

/** Drop the 60s status cache. Called by the reindex run after every collection
 *  it rebuilds, because the collection's live shape has just changed and
 *  `/alerts` reads this — a cached "needs reindex" surviving the rebuild that
 *  fixed it is an alarm that trains people to ignore alarms. */
export function invalidateUpgradeStatus(): void {
  statusCache = null
}

/** Rebuild every collection in the current model's shape (hybrid v2), then
 *  refill from the systems of record. Search runs thin between the swap and
 *  the backfill finishing — the admin UI says so before the button.
 *
 *  SIGNATURE UNCHANGED (`Promise<void>`, the caller fires it), body replaced: it
 *  enqueues a durable run whose checkpoint remembers which collections have
 *  already been rebuilt, so a deploy mid-rebuild resumes at the next collection
 *  instead of dropping the ones that were already refilled. `startReindex`
 *  refuses a second run while one is live. */
export const reindexAll = (): Promise<void> => startReindex()
