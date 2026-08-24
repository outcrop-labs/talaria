// THE PLAN-DRAFT RUN — "draft tickets from this conversation" as durable work,
// on the same runtime research and the workbench already run on.
//
// WHY A RUN AT ALL, when the old route did this synchronously in one POST: a
// draft is an agent reading a conversation for tens of seconds, and the human
// asked to be able to close the tab. A synchronous POST is a promise to stay
// on the line; a run is a row. The row survives the closed browser, the
// reloaded tab, and a server restart mid-draft (the reclaim sweep re-enters
// this step from the last checkpoint — there is none, so it simply runs
// again), and the review finds its way back by asking for the conversation's
// latest draft.
//
// ONE STEP, NO CHECKPOINTS. There is nothing to resume INTO: the step is one
// model call followed by one row write, and the checkpoint a caller would
// persist between them would be the model's reply itself — see the comment at
// the call.
import { registerRun, type RunDefinition, type StepResult } from '../define'
import type { Authority } from '../../approvals'
import { planFromChannel, planFromConversation, type TicketProposal } from '../../channel-plan'
import { db } from '../../db/pg'

export interface PlanDraftInput {
  conversationId: string
  /** Which transcript the agent reads — the two planners gather differently
   *  (channel messages vs a conversation with its living document). */
  source: 'plan' | 'channel'
  agentModel: string
  routedModel: string
  boardId: string | null
  templateId: string | null
}

/** What lands in `plan_drafts.proposals` — the reviewed shape, normalized once
 *  here so the row never holds a half-shaped batch. The review walk PATCHes
 *  this same shape back, which is why `include` starts life explicit. */
export type StoredProposal = TicketProposal & { include: boolean }

export interface PlanDraftDeps {
  draftTickets: (input: PlanDraftInput) => Promise<{ proposals: TicketProposal[]; raw: string }>
  saveResult: (id: string, out: { proposals: StoredProposal[]; note: string | null }) => Promise<void>
}

/** TicketProposal's arrays are required by type, but this is model output
 *  arriving as JSON — the synchronous route defended with the same `??`. */
const normalize = (p: TicketProposal): StoredProposal => ({
  ...p,
  dependsOn: p.dependsOn ?? [],
  tags: p.tags ?? [],
  include: true,
})

export const REAL_PLAN_DRAFT_DEPS: PlanDraftDeps = {
  draftTickets: (input) => {
    const tpl = { boardId: input.boardId, templateId: input.templateId }
    return input.source === 'channel'
      ? planFromChannel(input.conversationId, input.agentModel, input.routedModel, tpl)
      : planFromConversation(input.conversationId, input.agentModel, input.routedModel, tpl)
  },
  saveResult: async (id, out) => {
    const sql = await db()
    await sql`update plan_drafts set proposals = ${sql.json(out.proposals as never)}::jsonb,
      note = ${out.note}, updated_at = now() where id = ${id}`
  },
}

export function makePlanDraftRun(deps: PlanDraftDeps): RunDefinition<PlanDraftInput, null> {
  return {
    kind: 'plan-draft',
    label: 'Draft tickets',
    // One agent call over a conversation. The ceiling has to clear the
    // transport's worst case (the gateway's own timeout is ten minutes), not
    // the typical draft — a step that blows this is FILED AS AN ERROR, not
    // retried, because it is probably still running. The price is the lease
    // TTL: a driver killed mid-draft is reclaimable about five minutes later.
    maxStepMs: 5 * 60_000,
    // The drafter's own run: who may watch it and be told about it is the
    // person who clicked Draft tickets. (Same authority shape as research.)
    audience: (run): Authority => (run.ownerUserId ? { by: 'user', userIds: [run.ownerUserId] } : { by: 'admin' }),
    step: async (ctx): Promise<StepResult<null>> => {
      // AT-LEAST-ONCE (the checklist in define.ts): the model call bills on
      // re-entry. A driver that dies between the call and the row write below
      // will make the call once more on reclaim. Accepted deliberately: the
      // output is a draft the human regenerates at will, the repeat window is
      // one database write, and the alternative — checkpointing the model's
      // reply before persisting it — would still bill twice on every real
      // failure while saving only the rare reclaim.
      const out = await deps.draftTickets(ctx.input)
      // `raw` is for exactly one distinction, the same one the synchronous
      // route made: the agent ANSWERED but not in tickets, vs nothing to plan.
      const note = out.proposals.length === 0 ? (out.raw ? 'the agent did not return parseable tickets' : 'nothing to plan yet') : null
      await deps.saveResult(ctx.run.id, { proposals: out.proposals.map(normalize), note })
      return { kind: 'done', result: { count: out.proposals.length } }
    },
  }
}

export const planDraftRun = registerRun(makePlanDraftRun(REAL_PLAN_DRAFT_DEPS))
