// THE TWO RETRIEVAL REPAIR RUNS, on the durable runtime.
//
// WHAT THEY REPLACE, and it is the same bug twice:
//
//   retrieval/backfill.ts   `let backfillRunning = false` plus a status blob in
//   retrieval/migrate.ts    `app_settings`, driven by a bare `void fn()`. A
//                           deploy in the middle of either one leaves
//                           state:'running' in that blob FOREVER — nothing is
//                           driving it, nothing will ever notice, and the admin
//                           panel polls a row that will never change again. The
//                           only way out is to press the button a second time,
//                           which starts the whole thing from zero.
//
// So the status blob is gone and the `runs` row is the source of truth. The
// shapes the panel reads (`BackfillStatus`, `ReindexStatus`) are unchanged and
// are now PROJECTIONS of that row — see `backfillStatus` in retrieval/backfill.ts
// and `reindexStatus` in retrieval/migrate.ts.
//
// ── WHAT ONE STEP IS ─────────────────────────────────────────────────────────
//
// BACKFILL: one PAGE of one source. The checkpoint is a source plus a keyset
// cursor, so a resumed run re-indexes at most the page that was in flight —
// never the sources behind it. Keyset (`id > cursor`) rather than OFFSET on
// purpose: rows are written while a backfill runs, and an OFFSET page silently
// SKIPS a row for every insert that lands ahead of the cursor. A backfill that
// quietly misses documents is worse than one that is slow.
//
// REINDEX: one COLLECTION rebuilt, then the backfill's own steps. The rebuild
// is the most destructive thing in the product — it DROPS a Qdrant collection
// and purges the content-hash bookkeeping — so the re-entry rules below are
// written out rather than assumed.
//
// ── AT-LEAST-ONCE, AND WHY THESE TWO SURVIVE IT ──────────────────────────────
//
//   BACKFILL IS CONTENT-HASH IDEMPOTENT by construction: `syncKbDoc`,
//   `indexTicket` and `indexActivity` all no-op on an unchanged document. A page
//   that ran and did not checkpoint costs its embeddings a second time and
//   changes nothing else, which is the cheapest possible answer to rule 1.
//
//   THE REBUILD IS NOT, and the dangerous ordering is precise. Re-dropping a
//   collection that was JUST dropped and recreated is harmless — nothing has
//   refilled it yet. Re-dropping one the BACKFILL HAS ALREADY REFILLED destroys
//   work. The whole guard is therefore that the `rebuilding → backfilling` flip
//   is A STEP OF ITS OWN WITH NO OUTWARD EFFECT IN IT: the last collection's
//   rebuild checkpoints `rebuilt: [...all]`, and the step after it does nothing
//   but return the new phase. A crash anywhere in that window re-enters a step
//   that drops nothing. Put the flip in the same step as the last drop and the
//   window becomes "we rebuilt everything, the write was lost, now we rebuild
//   over a half-filled index" — which is the one failure this file exists to
//   make impossible.
//
//   THE DROP COMES BEFORE ITS CHECKPOINT, deliberately, and the alternative is
//   worse. Marking a collection rebuilt BEFORE dropping it would mean a crash in
//   between leaves a collection that was never rebuilt but is recorded as done:
//   its vectors keep the old dimension, every index and search call against it
//   goes on failing, and nothing will ever look at it again. A repeated drop
//   costs one empty collection recreated twice.
//
// ── WIRING ───────────────────────────────────────────────────────────────────
//
// THE RECLAIM SWEEP IS NOT IMPORTED HERE, and that is the reconcile decision
// rather than an omission. This file used to carry its own `import '../reclaim'`
// and two of the four definition modules did not, so whether a deploy could
// resume a crashed run depended on which admin route it happened to load.
// server/runs/boot.ts is now the ONE place that names the sweep and every kind,
// it is imported from the route graph, and 'run-reclaim' is in REQUIRED_JOBS in
// server/scheduler.ts — so an instance that cannot reclaim says so at boot
// instead of quietly not doing it.
//
// A NOTE ON THE IMPORT CYCLE with retrieval/backfill.ts and retrieval/migrate.ts.
// Those modules own the public verbs (`backfillAll`, `reindexAll`) and their
// status projections; this one owns the definitions and the steps. Each side
// imports the other, and it is safe because NEITHER READS THE OTHER AT MODULE
// EVALUATION TIME: the definitions below reference `ragHealth` and
// `invalidateUpgradeStatus` only inside `step`, and those modules touch the
// definitions only inside function bodies. Same hazard, and the same reasoning,
// as the deferred `pause` import in server/runs/run.ts.
import { db } from '../../db/pg'
import { getArtifact } from '../../artifacts'
import { describeAgent } from '../../gateway'
import { applyArtifactRouting } from '../../retrieval/artifact-routing'
import { ragHealth } from '../../retrieval/backfill'
import { ensureQdrantFor } from '../../retrieval/collections'
import { embedInfo, embedOne } from '../../retrieval/embed'
import { invalidateUpgradeStatus } from '../../retrieval/migrate'
import { deleteCollection, ensureHybridCollection } from '../../retrieval/qdrant'
import { EFFECTIVE_DOC_SELECT, indexActivity, indexPersonal, indexTicket, indexTicketComment, syncKbDoc, type KbDocSync } from '../../retrieval/sources'
import { defineRun, registerRun, type RunDefinition, type RunStepContext, type StepResult } from '../define'
import { enqueue } from '../run'
import { activeRunOfKind } from '../store'

const LOG = '[retrieval/runs]'

/** ONE PAGE. Big enough that the checkpoint write is noise against the
 *  embeddings it covers, small enough that a cancel lands within a minute or
 *  so and a reclaim re-buys at most this many embeddings. */
export const BACKFILL_PAGE = 100

/** How long one page (or one collection rebuild) may take before the driver
 *  abandons it. It is also the lease TTL, so a crashed driver's run is
 *  reclaimable roughly this long after it stops renewing — four minutes is a
 *  page of a hundred slow embeddings with room, and a recovery delay an admin
 *  watching a repair job will accept. */
const MAX_STEP_MS = 240_000

/** Come back when the retrieval services are up. A `retry` rather than an
 *  error: a dead Qdrant is a fact about the deployment, and failing the run for
 *  it would mean an admin has to notice the services came back AND remember to
 *  press the button again. The row stays visible with the reason as its phase. */
const SERVICES_DOWN_RETRY_MS = 60_000

/** The zero uuid, as the "before every row" cursor. Keyset paging needs a
 *  lower bound and `id > null` is null; coalescing in SQL keeps the predicate
 *  one expression instead of two query shapes per source. */
const NO_CURSOR = '00000000-0000-0000-0000-000000000000'

// ── The backfill checkpoint ──────────────────────────────────────────────────

/** The systems of record, in the order a backfill walks them. Order is part of
 *  the checkpoint's meaning — a resumed run continues at `source` and never
 *  revisits the ones behind it — so this array is the sequence, not a set. */
export type BackfillSource = 'collections' | 'kb-docs' | 'channel-messages' | 'tickets' | 'comments' | 'plan-turns' | 'artifacts'

export const BACKFILL_SOURCES: readonly BackfillSource[] = [
  'collections',
  'kb-docs',
  'channel-messages',
  'tickets',
  'comments',
  'plan-turns',
  'artifacts',
]

/** What a person reads while they wait. */
const SOURCE_LABEL: Record<BackfillSource, string> = {
  collections: 'preparing the collections',
  'kb-docs': 'knowledge base docs',
  'channel-messages': 'channel messages',
  tickets: 'tickets',
  comments: 'ticket comments',
  'plan-turns': 'plan turns',
  artifacts: 'docs and research',
}

export interface BackfillCheckpoint {
  /** The source the NEXT page comes from. */
  source: BackfillSource
  /** The id of the last row indexed in `source`; null at the start of one.
   *  Keyset, not an offset — see the file header. */
  cursor: string | null
  /** The per-source tally the admin panel prints. Carried in the checkpoint
   *  rather than recomputed, because "how much did this backfill actually
   *  index" is not derivable from a cursor. */
  counts: Record<string, number>
}

const FRESH_BACKFILL: BackfillCheckpoint = { source: 'collections', cursor: null, counts: {} }

const bump = (counts: Record<string, number>, key: string, n = 1): Record<string, number> => ({ ...counts, [key]: (counts[key] ?? 0) + n })

const nextSource = (source: BackfillSource): BackfillSource | null => {
  const at = BACKFILL_SOURCES.indexOf(source)
  return at < 0 ? null : (BACKFILL_SOURCES[at + 1] ?? null)
}

// ── One page of one source ───────────────────────────────────────────────────

interface Page {
  /** Nothing left in this source. */
  done: boolean
  /** The last id indexed, so a resume starts after it. Unchanged from the one
   *  handed in when the page indexed nothing. */
  cursor: string | null
  counts: Record<string, number>
}

/** Index one page. Every write goes through the SAME indexer an ordinary save
 *  runs, which is what makes a re-entered page free: the content hash makes an
 *  unchanged document a no-op.
 *
 *  `signal` is honored BEFORE every outward call, not merely awaited on. A step
 *  the driver abandoned (deadline, lost lease) keeps running — nothing can stop
 *  a promise that ignores its signal — and a page that went on embedding after
 *  another instance took the run is the doubled side effect this runtime exists
 *  to prevent. */
async function indexPage(source: BackfillSource, cursor: string | null, counts: Record<string, number>, signal: AbortSignal): Promise<Page> {
  const sql = await db()
  const from = cursor ?? NO_CURSOR
  let tally = counts
  let last = cursor

  if (source === 'collections') {
    // Every registered collection gets its Qdrant collection in its registered
    // shape — they may have been registered while Qdrant was down. A handful of
    // rows, so it is one step rather than a paged source.
    const dim = (await embedOne('dim probe')).length
    const cols = (await sql`select qdrant_name as name, schema_version as "schemaVersion" from rag_collections`) as unknown as Array<{
      name: string
      schemaVersion: number
    }>
    for (const c of cols) {
      if (signal.aborted) return { done: false, cursor: last, counts: tally }
      await ensureQdrantFor(c.name, c.schemaVersion, dim).catch(() => {})
    }
    return { done: true, cursor: null, counts: tally }
  }

  if (source === 'kb-docs') {
    // EFFECTIVE visibility, the same resolution every save runs
    // (kb.syncDocEffective). Reading `d.visibility` raw here is how private docs
    // got into the org brain: perms_inherited defaults true and visibility
    // defaults 'org', so a doc in a private space reads 'org' until you resolve
    // it through the space. The paging predicate rides on the shared SELECT
    // rather than replacing it — this loop is the batch twin of the live path
    // and the two must not disagree about who may see a document.
    const docs = (await sql.unsafe(
      `${EFFECTIVE_DOC_SELECT} where d.id > $1::uuid order by d.id asc limit ${BACKFILL_PAGE}`,
      [from],
    )) as unknown as KbDocSync[]
    for (const d of docs) {
      if (signal.aborted) return { done: false, cursor: last, counts: tally }
      await syncKbDoc(d).catch(() => {})
      tally = bump(tally, 'kbDocs')
      last = d.id
    }
    return { done: docs.length < BACKFILL_PAGE, cursor: last, counts: tally }
  }

  if (source === 'channel-messages') {
    // Relay summaries ride along — they are messages too.
    const msgs = (await sql`
      select m.id, m.channel_id as "channelId", m.author_type as "authorType", m.author, m.content, c.name
      from channel_messages m join channels c on c.id = m.channel_id
      where m.status = 'complete' and m.content <> '' and c.kind <> 'dm' and m.id > ${from}::uuid
      order by m.id asc limit ${BACKFILL_PAGE}
    `) as unknown as Array<{ id: string; channelId: string; authorType: string; author: string; content: string; name: string }>
    for (const m of msgs) {
      if (signal.aborted) return { done: false, cursor: last, counts: tally }
      const author = m.authorType === 'agent' ? describeAgent(m.author).label : m.author
      await indexActivity({
        sourceType: 'channel',
        sourceId: m.id,
        title: `#${m.name} · ${author}`,
        text: m.content,
        payload: { channelId: m.channelId },
        href: '/channels',
      }).catch(() => {})
      tally = bump(tally, 'channelMessages')
      last = m.id
    }
    return { done: msgs.length < BACKFILL_PAGE, cursor: last, counts: tally }
  }

  if (source === 'tickets') {
    const tasks = (await sql`
      select t.id, t.board_id as "boardId", t.title, t.description from tasks t
      where t.archived_at is null and t.id > ${from}::uuid
      order by t.id asc limit ${BACKFILL_PAGE}
    `) as unknown as Array<{ id: string; boardId: string; title: string; description: string | null }>
    for (const t of tasks) {
      if (signal.aborted) return { done: false, cursor: last, counts: tally }
      await indexTicket(t).catch(() => {})
      tally = bump(tally, 'tickets')
      last = t.id
    }
    return { done: tasks.length < BACKFILL_PAGE, cursor: last, counts: tally }
  }

  if (source === 'comments') {
    const comments = (await sql`
      select c.id, c.task_id as "taskId", t.board_id as "boardId", c.author, c.content
      from task_comments c join tasks t on t.id = c.task_id
      where t.archived_at is null and c.id > ${from}::uuid
      order by c.id asc limit ${BACKFILL_PAGE}
    `) as unknown as Array<{ id: string; taskId: string; boardId: string; author: string; content: string }>
    for (const c of comments) {
      if (signal.aborted) return { done: false, cursor: last, counts: tally }
      await indexTicketComment(c).catch(() => {})
      tally = bump(tally, 'comments')
      last = c.id
    }
    return { done: comments.length < BACKFILL_PAGE, cursor: last, counts: tally }
  }

  if (source === 'plan-turns') {
    const planMsgs = (await sql`
      select m.id, m.conversation_id as "planId", c.user_id as "ownerId", c.title, m.content,
             coalesce(u.name, u.email) as author
      from messages m
      join conversations c on c.id = m.conversation_id and c.kind = 'plan'
      left join users u on u.id = m.author_user_id
      where m.role = 'user' and m.content <> '' and m.id > ${from}::uuid
      order by m.id asc limit ${BACKFILL_PAGE}
    `) as unknown as Array<{ id: string; planId: string; ownerId: string; title: string | null; content: string; author: string | null }>
    for (const m of planMsgs) {
      if (signal.aborted) return { done: false, cursor: last, counts: tally }
      await indexActivity({
        sourceType: 'plan',
        sourceId: m.id,
        title: `Plan (${m.title || 'Untitled'}) · ${m.author ?? 'someone'}`,
        text: m.content,
        payload: { planId: m.planId, planOwnerId: m.ownerId },
        href: '/plan',
      }).catch(() => {})
      tally = bump(tally, 'planTurns')
      last = m.id
    }
    return { done: planMsgs.length < BACKFILL_PAGE, cursor: last, counts: tally }
  }

  // ARTIFACTS, PAGED BY ARTIFACT AND NOT BY LINK ROW. The pre-run query was one
  // left join over artifact_links, which yields a row per link — so a page
  // boundary landing inside an artifact with two links would advance the cursor
  // past the artifact and DROP the second link silently. Two queries keep the
  // page artifact-aligned and produce exactly the same set of (artifact, link)
  // pairs the join did.
  const arts = (await sql`
    select a.id, a.title, a.body, a.visibility, a.owner_user_id as "ownerId", a.rag_routing as "ragRouting"
    from artifacts a
    where a.kind = 'doc' and a.body <> '' and a.id > ${from}::uuid
    order by a.id asc limit ${BACKFILL_PAGE}
  `) as unknown as Array<{ id: string; title: string; body: string; visibility: string; ownerId: string | null; ragRouting: string }>
  const links =
    arts.length === 0
      ? []
      : ((await sql`
          select artifact_id as "artifactId", target_type as "targetType", target_id as "targetId"
          from artifact_links
          where artifact_id in ${sql(arts.map((a) => a.id))} and target_type in ('plan', 'research')
        `) as unknown as Array<{ artifactId: string; targetType: string; targetId: string }>)

  for (const a of arts) {
    if (signal.aborted) return { done: false, cursor: last, counts: tally }
    last = a.id
    // Routed artifacts (explicit brain / none) are placed by their routing, not
    // by the activity flows.
    if (a.ragRouting && a.ragRouting !== 'auto') {
      const full = await getArtifact(a.id)
      if (full) await applyArtifactRouting(full).catch(() => {})
      tally = bump(tally, 'routedArtifacts')
      continue
    }
    for (const l of links.filter((x) => x.artifactId === a.id)) {
      if (signal.aborted) return { done: false, cursor: last, counts: tally }
      if (l.targetType === 'plan') {
        await indexActivity({
          sourceType: 'plan-doc',
          sourceId: a.id,
          title: a.title,
          text: `${a.title}\n\n${a.body}`,
          payload: { planId: l.targetId, planOwnerId: a.ownerId },
          href: `/artifacts?a=${a.id}`,
        }).catch(() => {})
        tally = bump(tally, 'planDocs')
      } else if (l.targetType === 'research') {
        const doc = {
          sourceType: 'research',
          sourceId: a.id,
          title: a.title,
          text: `${a.title}\n\n${a.body}`,
          payload: a.ownerId ? { runId: l.targetId } : { runId: l.targetId, orgWide: true },
          href: `/research/${l.targetId}`,
        }
        if (a.visibility === 'private' && a.ownerId) await indexPersonal(a.ownerId, doc).catch(() => {})
        else if (a.visibility !== 'private') await indexActivity(doc).catch(() => {})
        tally = bump(tally, 'research')
      }
    }
  }
  return { done: arts.length < BACKFILL_PAGE, cursor: last, counts: tally }
}

// ── The backfill step, shared by both kinds ──────────────────────────────────

export type BackfillProgress =
  | { kind: 'next'; checkpoint: BackfillCheckpoint; phase: string }
  | { kind: 'done'; counts: Record<string, number> }
  | { kind: 'retry'; after: number; reason: string }

/** The two edges a backfill page touches outside itself, injected so a test
 *  drives the whole paging state machine with no Postgres, no Qdrant and no
 *  embedding service. Same pattern and same reason as `RunDeps` next door. */
export interface BackfillDeps {
  health: () => Promise<{ qdrant: boolean; embeddings: boolean }>
  page: (source: BackfillSource, cursor: string | null, counts: Record<string, number>, signal: AbortSignal) => Promise<Page>
}

// Wrapped in arrows rather than referenced directly, and it is not style: this
// literal is evaluated at MODULE LOAD, and `ragHealth` comes from the other half
// of the import cycle described in the header. It happens to be a hoisted
// `function` declaration today, so a bare reference works; the day somebody
// rewrites it as `const ragHealth = async () => …` a bare reference here becomes
// a temporal-dead-zone crash at boot on one import order and not the other.
const REAL_BACKFILL_DEPS: BackfillDeps = { health: () => ragHealth(), page: (source, cursor, counts, signal) => indexPage(source, cursor, counts, signal) }

/** ONE PAGE OF PROGRESS, or the reason there is none yet.
 *
 *  In this shape rather than as a `StepResult` because BOTH kinds run it: the
 *  backfill run IS this, and the reindex run's second phase is this wrapped in a
 *  checkpoint that also remembers which collections it rebuilt. Two copies of
 *  the paging would be two places for a source to be forgotten. */
export async function stepBackfill(
  prior: BackfillCheckpoint | null,
  signal: AbortSignal,
  deps: Partial<BackfillDeps> = {},
): Promise<BackfillProgress> {
  const d: BackfillDeps = { ...REAL_BACKFILL_DEPS, ...deps }
  const cp = prior ?? FRESH_BACKFILL

  // Checked at the START OF EACH SOURCE rather than on every page: the probe
  // costs an embedding call, and paying for one per hundred documents to
  // rediscover a service that was up a second ago is a tax on the healthy path.
  if (cp.cursor === null) {
    const health = await d.health()
    if (!health.qdrant || !health.embeddings) {
      return {
        kind: 'retry',
        after: SERVICES_DOWN_RETRY_MS,
        reason: `waiting for retrieval (qdrant: ${health.qdrant ? 'up' : 'down'}, embeddings: ${health.embeddings ? 'up' : 'down'}); resumes at ${SOURCE_LABEL[cp.source]}`,
      }
    }
  }

  const page = await d.page(cp.source, cp.cursor, cp.counts, signal)
  if (!page.done) {
    return {
      kind: 'next',
      checkpoint: { source: cp.source, cursor: page.cursor, counts: page.counts },
      phase: `${SOURCE_LABEL[cp.source]}: ${Object.values(page.counts).reduce((n, v) => n + v, 0)} indexed`,
    }
  }

  const after = nextSource(cp.source)
  if (!after) return { kind: 'done', counts: page.counts }
  return { kind: 'next', checkpoint: { source: after, cursor: null, counts: page.counts }, phase: SOURCE_LABEL[after] }
}

// ── The backfill run ─────────────────────────────────────────────────────────

export const BACKFILL_KIND = 'rag-backfill'

export const backfillRun = registerRun(
  defineRun<Record<string, never>, BackfillCheckpoint>({
    kind: BACKFILL_KIND,
    label: 'Re-index the workspace',
    maxStepMs: MAX_STEP_MS,
    // ORG-WIDE WORK WITH NO OWNER AND NO SUBJECT. Only an admin can start it and
    // only an admin should be able to see it stall.
    audience: () => ({ by: 'admin' }),
    step: (ctx) => stepBackfillRun(ctx),
  }),
)

export async function stepBackfillRun(
  ctx: RunStepContext<Record<string, never>, BackfillCheckpoint>,
  deps: Partial<BackfillDeps> = {},
): Promise<StepResult<BackfillCheckpoint>> {
  const res = await stepBackfill(ctx.checkpoint, ctx.signal, deps)
  if (res.kind === 'done') return { kind: 'done', result: { counts: res.counts } }
  if (res.kind === 'retry') return res
  return { kind: 'next', checkpoint: res.checkpoint, phase: res.phase }
}

// ── The reindex checkpoint and run ───────────────────────────────────────────

export interface ReindexCheckpoint {
  /** The same two words `ReindexStatus.phase` has always used, because the
   *  admin panel prints them and this row is now where they come from. */
  phase: 'rebuilding' | 'backfilling'
  /** `rag_collections.id` for every collection already rebuilt. The whole
   *  re-entry argument rests on this list and on the empty transition step that
   *  follows it — see the file header. */
  rebuilt: string[]
  /** The dimension the rebuild committed to, kept so the phase line can say it
   *  and so a resumed rebuild cannot silently switch models mid-run. */
  embedDim: number | null
  /** Null until the rebuild is finished. */
  backfill: BackfillCheckpoint | null
}

export const REINDEX_KIND = 'rag-reindex'

const FRESH_REINDEX: ReindexCheckpoint = { phase: 'rebuilding', rebuilt: [], embedDim: null, backfill: null }

export interface RegisteredCollection {
  id: string
  qdrantName: string
}

/** Every edge the rebuild touches, injected — `rebuild` above all, because the
 *  property worth a test here is "a reclaim does not drop a collection twice"
 *  and asserting it against a real Qdrant is not a unit test. */
export interface ReindexDeps {
  embedDim: () => Promise<number | null>
  collections: () => Promise<RegisteredCollection[]>
  /** THE DESTRUCTIVE UNIT, as one verb: drop the collection, recreate it in the
   *  new shape, purge the content-hash bookkeeping so nothing skips the
   *  re-embed, and stamp the registry. One verb rather than four because they
   *  are one step's worth of work and splitting them across steps would only add
   *  windows in which a collection exists in neither shape. */
  rebuild: (col: RegisteredCollection, dim: number) => Promise<void>
  invalidate: () => void
  backfill: BackfillDeps
}

const REAL_REINDEX_DEPS: ReindexDeps = {
  embedDim: async () => (await embedInfo())?.dim ?? null,
  collections: async () => {
    const sql = await db()
    // Ordered so "the next one not yet rebuilt" is a stable choice across
    // re-entries rather than whatever the planner returned this time.
    return (await sql`select id, qdrant_name as "qdrantName" from rag_collections order by id asc`) as unknown as RegisteredCollection[]
  },
  rebuild: async (col, dim) => {
    const sql = await db()
    await deleteCollection(col.qdrantName)
    await ensureHybridCollection(col.qdrantName, dim)
    // Old point ids went with the collection; drop the bookkeeping so content
    // hashes cannot skip the re-embed.
    await sql`delete from rag_points where collection_id = ${col.id}`
    await sql`update rag_collections set embed_dim = ${dim}, schema_version = 2 where id = ${col.id}`
  },
  // An arrow, for the reason spelled out over `REAL_BACKFILL_DEPS`: this literal
  // is evaluated at module load and `invalidateUpgradeStatus` is on the other
  // side of the import cycle.
  invalidate: () => invalidateUpgradeStatus(),
  backfill: REAL_BACKFILL_DEPS,
}

export async function stepReindex(
  ctx: RunStepContext<Record<string, never>, ReindexCheckpoint>,
  over: Partial<ReindexDeps> = {},
): Promise<StepResult<ReindexCheckpoint>> {
  const d: ReindexDeps = { ...REAL_REINDEX_DEPS, ...over }
  const cp = ctx.checkpoint ?? FRESH_REINDEX

  if (cp.phase === 'rebuilding') {
    const dim = await d.embedDim()
    if (dim === null) {
      // A `retry`, not an error. The pre-run code threw here and filed the whole
      // reindex as failed for the crime of TEI being down for a minute — and
      // then the admin had to notice it came back and press the button again.
      return { kind: 'retry', after: SERVICES_DOWN_RETRY_MS, reason: 'waiting for the embedding service before rebuilding' }
    }
    if (cp.embedDim !== null && cp.embedDim !== dim) {
      // THE EMBEDDING MODEL MOVED UNDER THE RUN. A dimension change is exactly
      // what this run exists to repair, so a change DURING it means every
      // collection rebuilt so far is now in the wrong shape. Start the rebuild
      // over at the new dimension rather than finishing it in two: half a plane
      // at 384 and half at 1024 fails every index and search call against the
      // wrong half, with nothing on /alerts that says which half or why.
      //
      // A pure step — it rewrites the checkpoint and drops nothing — so it is
      // safe to re-enter and cannot itself destroy anything.
      console.warn(`${LOG} ${ctx.run.id}: the embedding dimension changed from ${cp.embedDim} to ${dim} mid-rebuild — rebuilding every collection again`)
      return { kind: 'next', checkpoint: { ...cp, rebuilt: [], embedDim: dim }, phase: `the embedding dimension changed to ${dim}; starting the rebuild again` }
    }

    const cols = await d.collections()
    const rebuilt = new Set(cp.rebuilt)
    const next = cols.find((c) => !rebuilt.has(c.id))

    if (!next) {
      // THE TRANSITION, AND IT DOES NOTHING ELSE ON PURPOSE. This is the step
      // that makes a reclaim safe: between "every collection is rebuilt" and
      // "the checkpoint says backfilling" there is no outward effect at all, so
      // a driver that dies in that window re-enters a step that drops nothing.
      // Fold this into the last rebuild step and the window becomes "we rebuilt
      // everything, the write was lost, now we drop a half-refilled index" —
      // see the file header. (`invalidate` is a process-local cache reset, not
      // an outward effect: repeating it costs one status refetch.)
      d.invalidate()
      return { kind: 'next', checkpoint: { ...cp, phase: 'backfilling', embedDim: dim }, phase: 'refilling from the systems of record' }
    }

    if (ctx.signal.aborted) return { kind: 'retry', after: 0, reason: 'the driver gave this step up before the rebuild started' }
    if (ctx.attempt > 0 && cp.rebuilt.length > 0)
      console.warn(
        `${LOG} ${ctx.run.id}: resuming a rebuild at attempt ${ctx.attempt} — ${cp.rebuilt.length} of ${cols.length} collection(s) were already rebuilt and are NOT dropped again`,
      )

    // ONE COLLECTION, then the checkpoint.
    await d.rebuild(next, dim)
    d.invalidate()
    return {
      kind: 'next',
      checkpoint: { ...cp, rebuilt: [...cp.rebuilt, next.id], embedDim: dim },
      phase: `rebuilt ${next.qdrantName} (${cp.rebuilt.length + 1} of ${cols.length})`,
    }
  }

  const res = await stepBackfill(cp.backfill, ctx.signal, d.backfill)
  if (res.kind === 'done') return { kind: 'done', result: { counts: res.counts, rebuilt: cp.rebuilt.length } }
  if (res.kind === 'retry') return res
  return { kind: 'next', checkpoint: { ...cp, backfill: res.checkpoint }, phase: res.phase }
}

export const reindexRun = registerRun(
  defineRun<Record<string, never>, ReindexCheckpoint>({
    kind: REINDEX_KIND,
    label: 'Rebuild and refill the retrieval index',
    maxStepMs: MAX_STEP_MS,
    audience: () => ({ by: 'admin' }),
    step: (ctx) => stepReindex(ctx),
  }),
)

// ── Starting one ─────────────────────────────────────────────────────────────
//
// `latestRunOfKind` / `activeRunOfKind` / `KindRunView` USED TO LIVE HERE and
// are now in server/runs/store.ts, where the other two real queries over the
// table already live. They were never about retrieval: `fitness/surface.ts`
// imported them from this module, which made the model-fitness surface drag
// Qdrant, the embedder and every indexer into its graph to read one row.

/** ONE RUN PER THING, which is rule 6 of the at-least-once checklist: `enqueue`
 *  deduplicates nothing above the row, so a caller that retries its own POST —
 *  or an admin who presses the button twice — would otherwise start a SECOND
 *  backfill doing identical work against the same collections.
 *
 *  The check and the insert are two statements, so two presses landing in the
 *  same millisecond on two instances can still both pass. Named rather than
 *  hidden: the residual window is one round trip wide, both runs index the same
 *  content-hash-idempotent documents, and closing it properly wants a unique
 *  partial index on (kind) for the non-terminal states — a migration, which is
 *  not this workflow's to write. */
async function startOnce<C>(kind: string, def: RunDefinition<Record<string, never>, C>, phase: string): Promise<void> {
  const active = await activeRunOfKind(kind)
  if (active) {
    console.log(`${LOG} ${kind} is already running as ${active.id} ("${active.phase}") — showing that one rather than starting a second`)
    return
  }
  await enqueue(def, {}, { phase })
}

export const startBackfill = (): Promise<void> => startOnce(BACKFILL_KIND, backfillRun, 'queued')

export const startReindex = (): Promise<void> => startOnce(REINDEX_KIND, reindexRun, 'queued')
