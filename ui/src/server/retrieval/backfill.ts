// Retrieval health + repair. The indexing pipeline is fire-and-forget by
// design (a write must never block on RAG), which means a dead Qdrant/TEI
// fails SILENTLY — the brains just stop filling. Three defenses live here:
//   ragHealth()        cheap probes → /alerts (never silent again)
//   backfillAll()      full re-index of the workspace's history (admin button)
//   maybeRagSweep()    15-minute incremental catch-up over new rows, so an
//                      outage self-heals when the services come back
import { db } from '../db/pg'
import { getArtifact } from '../artifacts'
import { getSetting, setSetting } from '../audit'
import { describeAgent } from '../gateway'
import { applyArtifactRouting } from './artifact-routing'
import { ensureQdrantFor } from './collections'
import { embedOne } from './embed'
import { indexActivity, indexPersonal, indexTicket, indexTicketComment, syncKbDoc } from './sources'

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

export interface BackfillStatus {
  state: 'idle' | 'running' | 'done' | 'error'
  startedAt?: string
  finishedAt?: string
  counts?: Record<string, number>
  error?: string
}

const STATUS_KEY = 'rag_backfill_status'
export const backfillStatus = () => getSetting<BackfillStatus>(STATUS_KEY, { state: 'idle' })
const setStatus = (s: BackfillStatus) => setSetting(STATUS_KEY, s)

let backfillRunning = false

/** Full re-index of the workspace's history. Content-hash idempotent — safe to
 *  re-run; unchanged docs are no-ops. Detached-friendly (caller fires it). */
export async function backfillAll(): Promise<void> {
  if (backfillRunning) return
  backfillRunning = true
  const counts: Record<string, number> = {}
  try {
    await setStatus({ state: 'running', startedAt: new Date().toISOString() })
    const health = await ragHealth()
    if (!health.qdrant || !health.embeddings) {
      throw new Error(`retrieval services down (qdrant: ${health.qdrant}, embeddings: ${health.embeddings})`)
    }
    const sql = await db()

    // Every registered collection gets its Qdrant collection in its registered
    // shape (they may have been registered while Qdrant was down).
    const dim = (await embedOne('dim probe')).length
    const cols = (await sql`select qdrant_name as name, schema_version as "schemaVersion" from rag_collections`) as unknown as Array<{ name: string; schemaVersion: number }>
    for (const c of cols) await ensureQdrantFor(c.name, c.schemaVersion, dim).catch(() => {})

    // KB docs — visibility-routed via the same sync every save runs.
    const docs = (await sql`
      select d.id, d.space_id as "spaceId", d.title, d.body, d.visibility, d.official, d.owner_user_id as "ownerUserId"
      from kb_docs d
    `) as unknown as Array<{ id: string; spaceId: string; title: string; body: string; visibility: 'private' | 'org' | 'public'; official: boolean; ownerUserId: string | null }>
    for (const d of docs) {
      await syncKbDoc(d).catch(() => {})
      counts.kbDocs = (counts.kbDocs ?? 0) + 1
    }

    // Channel messages (relay summaries ride along — they're messages too).
    const msgs = (await sql`
      select m.id, m.channel_id as "channelId", m.author_type as "authorType", m.author, m.content, c.name, c.kind
      from channel_messages m join channels c on c.id = m.channel_id
      where m.status = 'complete' and m.content <> '' and c.kind <> 'dm'
    `) as unknown as Array<{ id: string; channelId: string; authorType: string; author: string; content: string; name: string; kind: string }>
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
      counts.channelMessages = (counts.channelMessages ?? 0) + 1
    }

    // Tickets + comments.
    const tasks = (await sql`
      select t.id, t.board_id as "boardId", t.title, t.description from tasks t where t.archived_at is null
    `) as unknown as Array<{ id: string; boardId: string; title: string; description: string | null }>
    for (const t of tasks) {
      await indexTicket(t).catch(() => {})
      counts.tickets = (counts.tickets ?? 0) + 1
    }
    const comments = (await sql`
      select c.id, c.task_id as "taskId", t.board_id as "boardId", c.author, c.content
      from task_comments c join tasks t on t.id = c.task_id where t.archived_at is null
    `) as unknown as Array<{ id: string; taskId: string; boardId: string; author: string; content: string }>
    for (const c of comments) {
      await indexTicketComment(c).catch(() => {})
      counts.comments = (counts.comments ?? 0) + 1
    }

    // Plan turns + org-visible doc/research artifacts (incl. plan docs).
    const planMsgs = (await sql`
      select m.id, m.conversation_id as "planId", c.user_id as "ownerId", c.title, m.content,
             coalesce(u.name, u.email) as author
      from messages m
      join conversations c on c.id = m.conversation_id and c.kind = 'plan'
      left join users u on u.id = m.author_user_id
      where m.role = 'user' and m.content <> ''
    `) as unknown as Array<{ id: string; planId: string; ownerId: string; title: string | null; content: string; author: string | null }>
    for (const m of planMsgs) {
      await indexActivity({
        sourceType: 'plan',
        sourceId: m.id,
        title: `Plan (${m.title || 'Untitled'}) · ${m.author ?? 'someone'}`,
        text: m.content,
        payload: { planId: m.planId, planOwnerId: m.ownerId },
        href: '/plan',
      }).catch(() => {})
      counts.planTurns = (counts.planTurns ?? 0) + 1
    }
    const arts = (await sql`
      select a.id, a.title, a.body, a.visibility, a.owner_user_id as "ownerId", a.rag_routing as "ragRouting",
             l.target_type as "targetType", l.target_id as "targetId"
      from artifacts a
      left join artifact_links l on l.artifact_id = a.id and l.target_type in ('plan', 'research')
      where a.kind = 'doc' and a.body <> ''
    `) as unknown as Array<{ id: string; title: string; body: string; visibility: string; ownerId: string | null; ragRouting: string; targetType: string | null; targetId: string | null }>
    for (const a of arts) {
      // Routed artifacts (explicit brain / none) are placed by their routing,
      // not the activity flows.
      if (a.ragRouting && a.ragRouting !== 'auto') {
        const full = await getArtifact(a.id)
        if (full) await applyArtifactRouting(full).catch(() => {})
        counts.routedArtifacts = (counts.routedArtifacts ?? 0) + 1
        continue
      }
      if (a.targetType === 'plan') {
        await indexActivity({
          sourceType: 'plan-doc',
          sourceId: a.id,
          title: a.title,
          text: `${a.title}\n\n${a.body}`,
          payload: { planId: a.targetId, planOwnerId: a.ownerId },
          href: '/artifacts',
        }).catch(() => {})
        counts.planDocs = (counts.planDocs ?? 0) + 1
      } else if (a.targetType === 'research') {
        const doc = {
          sourceType: 'research',
          sourceId: a.id,
          title: a.title,
          text: `${a.title}\n\n${a.body}`,
          payload: a.ownerId ? { runId: a.targetId } : { runId: a.targetId, orgWide: true },
          href: `/research?r=${a.targetId}`,
        }
        if (a.visibility === 'private' && a.ownerId) await indexPersonal(a.ownerId, doc).catch(() => {})
        else if (a.visibility !== 'private') await indexActivity(doc).catch(() => {})
        counts.research = (counts.research ?? 0) + 1
      }
    }

    await setStatus({ state: 'done', startedAt: undefined, finishedAt: new Date().toISOString(), counts })
  } catch (e) {
    await setStatus({ state: 'error', error: (e as Error).message, finishedAt: new Date().toISOString(), counts })
  } finally {
    backfillRunning = false
  }
}

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

  const docs = (await sql`
    select d.id, d.space_id as "spaceId", d.title, d.body, d.visibility, d.official, d.owner_user_id as "ownerUserId"
    from kb_docs d where d.updated_at > ${watermark}
  `) as unknown as Array<{ id: string; spaceId: string; title: string; body: string; visibility: 'private' | 'org' | 'public'; official: boolean; ownerUserId: string | null }>
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
      href: isPlan ? '/artifacts' : `/research?r=${a.targetId}`,
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
