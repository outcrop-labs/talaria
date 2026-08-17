// Retrieval health + repair. The indexing pipeline is fire-and-forget by
// design (a write must never block on RAG), which means a dead Qdrant/TEI
// fails SILENTLY — the brains just stop filling. Three defenses live here:
//   ragHealth()        cheap probes → /alerts (never silent again)
//   backfillAll()      full re-index of the workspace's history (admin button)
//   maybeRagSweep()    15-minute incremental catch-up over new rows, so an
//                      outage self-heals when the services come back
//
// THE BACKFILL IS A DURABLE RUN NOW. It used to be `let backfillRunning = false`
// plus a status blob in `app_settings`, fired as a bare `void backfillAll()`: a
// deploy in the middle left that blob saying state:'running' FOREVER, with
// nothing driving it and nothing that would ever notice. The work — the paging,
// the checkpoint, the resume — is in server/runs/defs/reindex.ts; what stays
// here is the public verb and the READ SHAPE the admin panel has always
// consumed, which is now a projection of the run row rather than a second copy
// of the truth. `components/admin/retrieval.ts` did not change.
import { db } from '../db/pg'
import { getSetting, setSetting } from '../audit'
import { getArtifact } from '../artifacts'
import { describeAgent } from '../gateway'
import { applyArtifactRouting } from './artifact-routing'
import { embedOne } from './embed'
import { EFFECTIVE_DOC_SELECT, indexActivity, indexPersonal, indexTicket, syncKbDoc, type KbDocSync } from './sources'
import { BACKFILL_KIND, startBackfill, type BackfillCheckpoint } from '../runs/defs/reindex'
import { latestRunOfKind, type KindRunView } from '../runs/store'

const QDRANT_URL = () => (process.env.TALARIA_QDRANT_URL ?? 'http://localhost:6333').replace(/\/$/, '')

export interface RagHealth {
  qdrant: boolean
  embeddings: boolean
}

/** Probe both retrieval services (cheap, ~2s worst case). */
export async function ragHealth(): Promise<RagHealth> {
  const [qdrant, embeddings] = await Promise.all([
    fetch(`${QDRANT_URL()}/collections`, { signal: AbortSignal.timeout(2_500) })
      .then((r) => r.ok)
      .catch(() => false),
    embedOne('health probe')
      .then((v) => v.length > 0)
      .catch(() => false),
  ])
  return { qdrant, embeddings }
}

/** THE READ SHAPE, unchanged: `components/admin/retrieval.ts` declares exactly
 *  these fields and polls while `state === 'running'`. What changed is where the
 *  answer comes from — a `runs` row rather than an `app_settings` blob nothing
 *  was driving. */
export interface BackfillStatus {
  state: 'idle' | 'running' | 'done' | 'error'
  startedAt?: string
  finishedAt?: string
  counts?: Record<string, number>
  error?: string
}

/** Whatever the run has indexed so far. Read off the CHECKPOINT while it runs
 *  and off the RESULT once it is done, which are the same numbers a step apart —
 *  the driver persists the checkpoint before it takes the next step, so the
 *  panel's tally can lag by one page and can never overstate. */
function countsOf(run: KindRunView): Record<string, number> | undefined {
  const done = run.result as { counts?: Record<string, number> } | null
  if (done?.counts) return done.counts
  const cp = run.checkpoint as BackfillCheckpoint | null
  return cp?.counts
}

/** Project a run row onto the four states the panel knows.
 *
 *  `queued` reads as RUNNING, and that is not a fudge: a queued run is one the
 *  reclaim sweep will pick up within thirty seconds, or one a `retry` has parked
 *  for a minute because Qdrant is down — in both cases the work is in flight
 *  from the admin's point of view and the reason is in `phase`. Showing it as
 *  idle would put the Start button back in front of somebody whose backfill is
 *  about to resume, which is how you get two.
 *
 *  `cancelled` reads as DONE with the reason attached. The panel has no fifth
 *  state and a stopped repair job is finished, not broken. */
function projectStatus(run: KindRunView | null): BackfillStatus {
  if (!run) return { state: 'idle' }
  const counts = countsOf(run)
  const at = { ...(run.startedAt ? { startedAt: run.startedAt } : {}), ...(run.finishedAt ? { finishedAt: run.finishedAt } : {}) }
  const tally = counts ? { counts } : {}
  if (run.state === 'error') return { state: 'error', error: run.error ?? 'the backfill failed', ...at, ...tally }
  if (run.state === 'cancelled') return { state: 'done', ...(run.error ? { error: run.error } : {}), ...at, ...tally }
  if (run.state === 'done') return { state: 'done', ...at, ...tally }
  return { state: 'running', ...at, ...tally }
}

export const backfillStatus = async (): Promise<BackfillStatus> => projectStatus(await latestRunOfKind(BACKFILL_KIND).catch(() => null))

/** Full re-index of the workspace's history. Content-hash idempotent — safe to
 *  re-run; unchanged docs are no-ops.
 *
 *  SIGNATURE UNCHANGED (`Promise<void>`, detached-friendly, the caller fires
 *  it), body replaced: it enqueues a durable run instead of starting an
 *  in-process loop nothing outlives. `startBackfill` refuses to open a second
 *  one while the first is live — the two-presses rule this function used to
 *  express with a module-level boolean that only worked on one instance. */
export const backfillAll = (): Promise<void> => startBackfill()

// ── Incremental catch-up sweep ────────────────────────────────────────────────
// Event-driven indexing is the primary path; this 15-minute sweep re-indexes
// anything CREATED/UPDATED since the last high-water mark, so rows written
// while the services were down get picked up when they return. Content hashes
// make the overlap free.
const SWEEP_KEY = 'rag_sweep_watermark'
const SWEEP_INTERVAL_MS = 15 * 60_000
let lastSweepAt = 0

export async function sweepNewActivity(): Promise<number> {
  const health = await ragHealth()
  if (!health.qdrant || !health.embeddings) return 0
  const sql = await db()
  const watermark = await getSetting<string>(SWEEP_KEY, new Date(0).toISOString())
  const now = new Date().toISOString()
  let indexed = 0

  // Effective visibility, exactly as the live save path resolves it — a doc
  // inheriting from a private space must never reach the org brain.
  const docs = (await sql.unsafe(`${EFFECTIVE_DOC_SELECT} where d.updated_at > $1`, [watermark])) as unknown as KbDocSync[]
  for (const d of docs) {
    await syncKbDoc(d).catch(() => {})
    indexed++
  }
  const msgs = (await sql`
    select m.id, m.channel_id as "channelId", m.author_type as "authorType", m.author, m.content, c.name
    from channel_messages m join channels c on c.id = m.channel_id
    where m.status = 'complete' and m.content <> '' and c.kind <> 'dm' and m.created_at > ${watermark}
  `) as unknown as Array<{ id: string; channelId: string; authorType: string; author: string; content: string; name: string }>
  for (const m of msgs) {
    const author = m.authorType === 'agent' ? describeAgent(m.author).label : m.author
    await indexActivity({
      sourceType: 'channel',
      sourceId: m.id,
      title: `#${m.name} · ${author}`,
      text: m.content,
      payload: { channelId: m.channelId },
      href: '/channels',
    }).catch(() => {})
    indexed++
  }
  const tasks = (await sql`
    select t.id, t.board_id as "boardId", t.title, t.description from tasks t
    where t.archived_at is null and t.updated_at > ${watermark}
  `) as unknown as Array<{ id: string; boardId: string; title: string; description: string | null }>
  for (const t of tasks) {
    await indexTicket(t).catch(() => {})
    indexed++
  }
  const arts = (await sql`
    select a.id, a.title, a.body, a.visibility, a.owner_user_id as "ownerId", a.rag_routing as "ragRouting",
           l.target_type as "targetType", l.target_id as "targetId"
    from artifacts a
    left join artifact_links l on l.artifact_id = a.id and l.target_type in ('plan', 'research')
    where a.kind = 'doc' and a.body <> '' and a.updated_at > ${watermark} and l.target_type is not null
  `) as unknown as Array<{ id: string; title: string; body: string; visibility: string; ownerId: string | null; ragRouting: string; targetType: string | null; targetId: string | null }>
  for (const a of arts) {
    if (a.ragRouting && a.ragRouting !== 'auto') {
      const full = await getArtifact(a.id)
      if (full) await applyArtifactRouting(full).catch(() => {})
      indexed++
      continue
    }
    const isPlan = a.targetType === 'plan'
    const doc = {
      sourceType: isPlan ? 'plan-doc' : 'research',
      sourceId: a.id,
      title: a.title,
      text: `${a.title}\n\n${a.body}`,
      payload: isPlan ? { planId: a.targetId, planOwnerId: a.ownerId } : a.ownerId ? { runId: a.targetId } : { runId: a.targetId, orgWide: true },
      href: isPlan ? '/artifacts' : `/research/${a.targetId}`,
    }
    if (!isPlan && a.visibility === 'private') {
      if (a.ownerId) await indexPersonal(a.ownerId, doc).catch(() => {})
    } else {
      await indexActivity(doc).catch(() => {})
    }
    indexed++
  }

  await setSetting(SWEEP_KEY, now)
  return indexed
}

/** Opportunistic scheduling (mirrors maybeSweepIdleChats): any comms/search
 *  read may kick a sweep, at most every 15 minutes, never blocking. */
export function maybeRagSweep(): void {
  const now = Date.now()
  if (now - lastSweepAt < SWEEP_INTERVAL_MS) return
  lastSweepAt = now
  void sweepNewActivity().catch(() => {})
}
