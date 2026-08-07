// Plan chat: turn a channel conversation into ticket proposals. A chosen
// channel agent reads the transcript and drafts structured tickets; a human
// reviews, edits, and creates them (into inbox — planning never assigns).
//
// The prompt, the output schema and the coercion now live in
// harness/defs/channel-plan.ts and run through `runHarness`. What went away
// here: `extractProposals` and `parseArrayAt` (one of the six hand-written
// structured-output extractors the audit found — the runner's `json.ts` scans
// balanced spans, strips fences and prefers fenced content over prose, and it
// repairs once with the concrete parser error, which nothing in this tree did),
// and the bare unguarded `proxyChat` call whose output becomes ticket bodies.
// What stays here: gathering the transcript, the template and the workflow map,
// because those are database reads and a harness definition must stay pure.
import { describeAgent } from './gateway'
import { channelPlanHarness, type ChannelPlanInput, type TicketProposal } from './harness/defs/channel-plan'
import { runHarness } from './harness/run'
import { listChannelMessages } from './channels'
import { priorMessages } from './conversations'
import { planDocFor, planTier } from './plan-doc'
import { resolveTemplate, templatePrompt } from './templates'
import { routingContext } from './workflows'

export type { TicketProposal }

/** Draft ticket proposals from a transcript. Shared by the channel Plan button
 *  and the first-class Plan surface.
 *
 *  `raw` is the model's reply, and both callers use it for exactly one thing:
 *  telling "the agent did not return parseable tickets" apart from "nothing to
 *  plan yet". The runner bounds it (8k) because a run result is not an archive;
 *  that is invisible to a truthiness check. */
async function planFromTranscript(
  transcript: string,
  agentModel: string,
  routedModel: string,
  refId: string,
  source: 'channel' | 'chat',
  opts: {
    /** The plan's living document — the authoritative source when present. */
    planDoc?: string
    /** The resolved ticket template — descriptions must follow its skeleton. */
    templatePrompt?: string
  } = {},
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  // Ahead of the harness on purpose: a conversation with nothing in it has no
  // tickets in it, and this early-out is what stops the Plan button spending a
  // model call and a harness_runs row to discover that.
  if (!transcript.trim() && !opts.planDoc?.trim()) return { proposals: [], raw: '' }

  const input: ChannelPlanInput = {
    transcript,
    ...(opts.planDoc?.trim() ? { planDoc: opts.planDoc } : {}),
    ...(opts.templatePrompt ? { templatePrompt: opts.templatePrompt } : {}),
    // Never fatal: an org with no workflows, or a failed read, just means no
    // routing labels. Same `.catch` this call has always had.
    ...(await routingContext()
      .then((map) => (map ? { routingMap: map } : {}))
      .catch(() => ({}))),
  }

  // The CHOSEN agent drafts, so the model is pinned rather than resolved from a
  // chain — and the tier is named apart from the base agent because the runner
  // needs both halves: it calls `<agent>-<alias>` and meters the spend against
  // `<agent>` (see `planTier`). No transport is supplied: the runner routes a
  // persona tier itself and carries the attribution on the request.
  const tier = planTier(agentModel, routedModel)
  const result = await runHarness(channelPlanHarness, input, {
    caller: `${source}:${refId}`,
    model: agentModel,
    ...(tier ? { tier } : {}),
    ledger: { source, refId },
  })
  // A run that never REACHED a model is not "nothing to plan yet". Both routes
  // pick between "the agent did not return parseable tickets" and "nothing to
  // plan yet", and a transport failure is neither — so a restarting agent
  // container answered 200 "nothing to plan yet" on a channel full of work.
  // Pre-port this threw `gateway error 502` and the routes turned it into a 502
  // with that sentence; `plan-doc.ts` draws the same distinction the same way.
  //
  // `answered` is that question with a name on it. This used to ask `raw ===
  // null`, which is a DRILL-DOWN field standing in for a control-flow fact, and
  // it got one case wrong in both directions: a stream that died after three
  // tokens leaves a `raw` behind and read here as a model that answered badly.
  // A model that genuinely answered badly still falls through to the
  // parseable-tickets note, which is what this test is for.
  if (!result.answered && result.error) throw new Error(result.error)
  return { proposals: result.value ?? [], raw: result.raw ?? '' }
}

/** Template context for a draft: where the tickets will land + any explicit
 *  pick. Resolution (explicit → agent → board default → none) happens here. */
export interface DraftTemplateCtx {
  boardId?: string | null
  templateId?: string | null
}

/** The ticket template as the model is told about it, or nothing. Resolution is
 *  a database read and stays on this side of the harness boundary. */
async function templateBlock(agentModel: string, tpl: DraftTemplateCtx): Promise<string | undefined> {
  const template = await resolveTemplate('ticket', { explicitId: tpl.templateId, agentModel, boardId: tpl.boardId })
  return template ? templatePrompt(template, 'ticket descriptions') : undefined
}

export async function planFromChannel(
  channelId: string,
  agentModel: string,
  routedModel: string,
  tpl: DraftTemplateCtx = {},
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  const history = await listChannelMessages(channelId, -1, 80)
  const transcript = history
    .filter((m) => m.status === 'complete' && m.content)
    .map((m) => `${m.authorType === 'agent' ? describeAgent(m.author).label : m.author}: ${m.content}`)
    .join('\n\n')
  const block = await templateBlock(agentModel, tpl)
  return planFromTranscript(transcript, agentModel, routedModel, channelId, 'channel', {
    ...(block ? { templatePrompt: block } : {}),
  })
}

/** Draft tickets from a plan conversation (the Plan surface). The plan's living
 *  document, when it has content, is the primary source; chat is context. */
export async function planFromConversation(
  conversationId: string,
  agentModel: string,
  routedModel: string,
  tpl: DraftTemplateCtx = {},
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  const label = describeAgent(agentModel).label
  const msgs = await priorMessages(conversationId)
  const transcript = msgs
    .filter((m) => m.content)
    .map((m) => `${m.role === 'assistant' ? label : 'User'}: ${m.content}`)
    .join('\n\n')
  const doc = await planDocFor(conversationId)
  const block = await templateBlock(agentModel, tpl)
  return planFromTranscript(transcript, agentModel, routedModel, conversationId, 'chat', {
    ...(doc?.body ? { planDoc: doc.body } : {}),
    ...(block ? { templatePrompt: block } : {}),
  })
}
