// PLAN DRAFTS — the domain half of the durable ticket-draft job. The run row
// (kind 'plan-draft', same uuid) owns the state machine; this module owns the
// row the review reads: starting one, finding the conversation's latest,
// saving the walk's edits back, dropping a consumed or discarded batch.
//
// ONE DRAFT PER CONVERSATION AT A TIME. The single-flight check is read-then-
// write, so two truly concurrent POSTs can both pass it and enqueue two runs —
// the modal prevents this client-side and the loser simply overwrites the
// conversation's "latest" slot; the row never duplicates a TICKET, because
// nothing is created until the human reviews.
import { randomUUID } from 'node:crypto'
import { db } from './db/pg'
import { cancelRun, drive, enqueue } from './runs/run'
import { planDraftRun, type PlanDraftInput, type StoredProposal } from './runs/defs/plan-draft'

export type { StoredProposal }

export interface PlanDraft {
  id: string
  conversationId: string
  source: 'plan' | 'channel'
  agentModel: string
  boardId: string | null
  templateId: string | null
  proposals: StoredProposal[]
  note: string | null
  /** The joined run's state. 'awaiting' cannot happen to this definition (it
   *  never parks on a decision) but a missing run row can — that reads as
   *  'done' so the proposals stay reviewable rather than vanishing. */
  status: 'queued' | 'running' | 'done' | 'error' | 'cancelled'
  phase: string
  error: string | null
  createdAt: string
}

interface DraftRow {
  id: string
  conversationId: string
  source: 'plan' | 'channel'
  agentModel: string
  boardId: string | null
  templateId: string | null
  proposals: StoredProposal[]
  note: string | null
  createdAt: string
  runState: string | null
  phase: string | null
  error: string | null
}

const toDraft = (r: DraftRow): PlanDraft => ({
  id: r.id,
  conversationId: r.conversationId,
  source: r.source,
  agentModel: r.agentModel,
  boardId: r.boardId,
  templateId: r.templateId,
  proposals: r.proposals ?? [],
  note: r.note,
  status: r.runState === 'awaiting' ? 'running' : r.runState === null ? 'done' : (r.runState as PlanDraft['status']),
  phase: r.phase ?? '',
  error: r.error ?? null,
  createdAt: r.createdAt,
})

/** The conversation's latest draft — the review's way back after a close, a
 *  reload, or a server restart. `null` means "nothing paired to this plan". */
export async function latestDraftFor(conversationId: string): Promise<PlanDraft | null> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.conversation_id as "conversationId", d.source, d.agent_model as "agentModel",
      d.board_id as "boardId", d.template_id as "templateId", d.proposals, d.note,
      d.created_at as "createdAt", r.state as "runState", r.phase, r.error
    from plan_drafts d
    left join runs r on r.id = d.id and r.kind = 'plan-draft'
    where d.conversation_id = ${conversationId}
    order by d.created_at desc
    limit 1
  `) as unknown as DraftRow[]
  return rows[0] ? toDraft(rows[0]) : null
}

async function activeDraftFor(conversationId: string): Promise<PlanDraft | null> {
  const sql = await db()
  const rows = (await sql`
    select d.id, d.conversation_id as "conversationId", d.source, d.agent_model as "agentModel",
      d.board_id as "boardId", d.template_id as "templateId", d.proposals, d.note,
      d.created_at as "createdAt", r.state as "runState", r.phase, r.error
    from plan_drafts d
    join runs r on r.id = d.id and r.kind = 'plan-draft'
    where d.conversation_id = ${conversationId} and r.state in ('queued', 'running', 'awaiting')
    order by d.created_at desc
    limit 1
  `) as unknown as DraftRow[]
  return rows[0] ? toDraft(rows[0]) : null
}

/** Start (or ride) the conversation's draft. Returns the projected draft —
 *  queued or running; the POST answers with it immediately. */
export async function startPlanDraft(args: {
  conversationId: string
  source: 'plan' | 'channel'
  userId: string
  agentModel: string
  routedModel: string
  tier: string | null
  boardId: string | null
  templateId: string | null
}): Promise<PlanDraft> {
  const existing = await activeDraftFor(args.conversationId)
  if (existing) return existing
  const id = randomUUID()
  const input: PlanDraftInput = {
    conversationId: args.conversationId,
    source: args.source,
    agentModel: args.agentModel,
    routedModel: args.routedModel,
    boardId: args.boardId,
    templateId: args.templateId,
  }
  // `start: false`, then the domain row, then drive — research's order: the
  // first step must not race the row it reports into, or the caller is the one
  // who loses.
  await enqueue(planDraftRun, input, {
    id,
    ownerUserId: args.userId,
    subjectType: 'plan-draft',
    subjectId: id,
    phase: 'queued',
    start: false,
  })
  const sql = await db()
  await sql`
    insert into plan_drafts (id, conversation_id, created_by, source, agent_model, routed_model, tier, board_id, template_id)
    values (${id}, ${args.conversationId}, ${args.userId}, ${args.source}, ${args.agentModel}, ${args.routedModel}, ${args.tier}, ${args.boardId}, ${args.templateId})
  `
  // The detached drive is the nicety; the reclaim sweep is the guarantee.
  void drive(id).catch((e) => console.error('[plan-draft] detached drive of', id, 'threw:', e))
  const created = await latestDraftFor(args.conversationId)
  if (!created) throw new Error('could not create the plan draft')
  return created
}

/** The review walk's edits, persisted: proposals written back to the latest
 *  draft so ticks, drops and retitles survive a reload of the review. */
export async function saveDraftProposals(conversationId: string, proposals: StoredProposal[]): Promise<void> {
  const sql = await db()
  await sql`
    update plan_drafts set proposals = ${sql.json(proposals as never)}::jsonb, updated_at = now()
    where id = (select id from plan_drafts where conversation_id = ${conversationId} order by created_at desc limit 1)
  `
}

/** Drop the conversation's draft — Back to config, or the batch was created
 *  and consumed. A still-live run is cancelled first so it stops at its next
 *  boundary instead of landing proposals into a deleted row. */
export async function dropDraft(conversationId: string): Promise<void> {
  const sql = await db()
  const rows = (await sql`
    select id from plan_drafts where conversation_id = ${conversationId} order by created_at desc limit 1
  `) as unknown as Array<{ id: string }>
  const latest = rows[0]
  if (!latest) return
  await cancelRun({ runId: latest.id, reason: 'draft discarded' }).catch(() => {})
  await sql`delete from plan_drafts where id = ${latest.id}`
}
