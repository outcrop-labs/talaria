// The plan's living document, server-side. The document IS a `doc` artifact
// linked to the plan conversation (artifact_links target_type='plan') — no
// separate model. This module finds/creates that artifact, lets the plan's own
// agent rewrite it from the conversation, keeps it in the activity index, and
// notifies teammates the plan @mentions (only ones who can read the document).
//
// The rewrite prompt, the reply's contract and the data-loss guard now live in
// harness/defs/plan-doc.ts and run through `runHarness`. Read that file's
// header before touching `syncPlanDoc`: the model is asked for the WHOLE
// document, so a truncated or gutted reply does not produce a worse document,
// it destroys a good one — and before the port the only thing standing between
// the two was a check for a completely empty string.
import {
  agentCategoryFolder,
  artifactsForTarget,
  attachArtifact,
  createArtifact,
  guarded,
  saveArtifact,
  type Artifact,
} from './artifacts'
import { listPlanMembers, priorMessages } from './conversations'
import { describeAgent } from './gateway'
import { planDocHarness, planDocRegression, type PlanDocInput } from './harness/defs/plan-doc'
import { runHarness } from './harness/run'
import { canRead, listEditors, setEditors } from './kb-perms'
import { notifyMentions } from './mentions'
import { indexActivity } from './retrieval/sources'
import { resolveTemplate, templatePrompt } from './templates'
import { routingContext } from './workflows'
import { listUsers } from './users'

/** The plan-mode harness, prepended to every plan-conversation turn. Without
 *  it the agent treats a planning chat like any other request and starts
 *  CREATING things (tickets, docs) — planning must stay side-effect free. */
export const PLAN_MODE_PROMPT = `This is a PLANNING conversation on the Plan surface. Your job is to think and decide WITH the teammate: clarify the goal, surface options and risks, and converge on scope, steps, and owners. A living plan document sits beside this chat and is rewritten from the conversation after each of your turns, so put decisions and structure into your words here.
Planning is side-effect free. Do NOT create or modify anything: no tickets, no documents or artifacts, no knowledge-base entries or spaces, no emails, calendar events, or channel posts. Reading is encouraged (search knowledge, read docs, list boards and tickets) to ground the plan in what actually exists.
When the plan is settled, the teammate turns it into tickets with the "Draft tickets" control on this surface. If asked to create tickets or other work products here, point to that control instead of doing it yourself.`

/** Routing awareness for plan surfaces: the org's workflow map, framed as a
 *  FINAL aside — never something that reshapes the plan itself. */
export async function planRoutingBlock(): Promise<string> {
  const ctx = await routingContext()
  if (!ctx) return ''
  return `\n\nThe org routes ticket work through workflows (match rules → skills → agents):\n${ctx}\nWhen converging on owners, prefer routing work where a workflow already covers it — and say so in passing, not as the plan's centerpiece.`
}

/** The plan's linked doc artifact, if one exists yet. */
export async function planDocFor(conversationId: string): Promise<Artifact | null> {
  const linked = await artifactsForTarget('plan', conversationId)
  return linked.find((a) => a.kind === 'doc') ?? null
}

/** Find-or-create the plan's document, seeded from the plan's template (the
 *  explicit per-plan pick if set, else the agent's bound plan template) — the
 *  skeleton is the starting structure. Owned by the plan's owner. */
export async function ensurePlanDoc(
  conversationId: string,
  owner: { id: string; label: string },
  planTitle: string | null,
  agentModel?: string,
  templateId?: string | null,
): Promise<Artifact> {
  const existing = await planDocFor(conversationId)
  if (existing) return existing
  const template = agentModel || templateId ? await resolveTemplate('plan', { explicitId: templateId, agentModel }) : null
  const artifact = await createArtifact({
    kind: 'doc',
    title: `Plan — ${planTitle || 'Untitled'}`,
    createdBy: owner.label,
    ownerUserId: owner.id,
    // Filed under the plan agent's cabinet, not dumped at the root.
    folderId: agentModel ? await agentCategoryFolder(describeAgent(agentModel).label, 'Plans', owner.label) : null,
  })
  await attachArtifact(artifact.id, { targetType: 'plan', targetId: conversationId }, owner.label)
  // Collaborators already on the plan get editor grants on the doc the moment
  // it exists (later shares grant at share time).
  const collaborators = (await listPlanMembers(conversationId)).filter((m) => m.role === 'collaborator')
  if (collaborators.length) {
    await setEditors('artifact', artifact.id, collaborators.map((m) => ({ principalType: 'user' as const, principalId: m.userId, role: 'editor' as const })))
  }
  if (template?.body.trim()) {
    return (await saveArtifact(artifact.id, { body: template.body }, owner.label)) ?? artifact
  }
  return artifact
}

/** Keep the activity brain current on a plan document (ACL: the plan's owner).
 *  Respects the artifact's routing — 'none'/explicit-brain docs stay out of
 *  the activity brain (retrieval/artifact-routing owns those placements). */
export async function indexPlanDoc(doc: Artifact, conversationId: string): Promise<void> {
  if (doc.ragRouting && doc.ragRouting !== 'auto') return
  await indexActivity({
    sourceType: 'plan-doc',
    sourceId: doc.id,
    title: doc.title,
    text: `${doc.title}\n\n${doc.body}`,
    payload: { planId: conversationId, planOwnerId: doc.ownerUserId },
    href: '/artifacts',
  })
}

/** The alias NAME of a routed persona id, or null when no tier was picked — the
 *  inverse of `routedModelFor`, which is the only thing that builds one
 *  (`${agentModel}-${tier}`, or `agentModel` unchanged). Both plan surfaces
 *  arrive holding the PAIR, and `RunContext.tier` wants the two halves apart.
 *
 *  A function rather than a slice at each call site because getting it wrong is
 *  invisible rather than loud: `recordUsage` prices a row by finding
 *  `agent_defs.model = agentModel` and then the alias named by `tier`, so a run
 *  handed "dex-developer-opus" as its model with no tier misses BOTH lookups —
 *  the row lands on an agent that does not exist, with no endpoint class, which
 *  means no price. A plan drafted on a tier would quietly be free. This was the
 *  second of the two gaps `plan-persona-turn.ts` existed to work around; the
 *  runner carries the attribution itself now, and only needs to be told the two
 *  names separately. */
export const planTier = (agentModel: string, routedModel: string): string | null =>
  routedModel === agentModel ? null : routedModel.slice(agentModel.length + 1)

/** Rewrite the plan document from the conversation, via the plan's own agent
 *  (persona gateway → metered like any chat turn). Returns the saved artifact.
 *
 *  THIS FUNCTION OVERWRITES A DOCUMENT A TEAM HAS BEEN BUILDING, and every
 *  refusal below exists for that. It throws rather than returning the unchanged
 *  artifact so the Plan surface can say what happened — the route already maps a
 *  throw to a 502 with this message, and silently returning the old document
 *  would show a "synced" document that never synced. */
export async function syncPlanDoc(
  conversationId: string,
  owner: { id: string; label: string },
  planTitle: string | null,
  agentModel: string,
  routedModel: string,
  templateId?: string | null,
): Promise<Artifact> {
  const doc = await ensurePlanDoc(conversationId, owner, planTitle, agentModel, templateId)
  const label = describeAgent(agentModel).label
  const msgs = await priorMessages(conversationId)
  const transcript = msgs
    .filter((m) => m.content)
    .map((m) => `${m.role === 'assistant' ? label : 'User'}: ${m.content}`)
    .join('\n\n')
  if (!transcript.trim()) return doc

  const template = await resolveTemplate('plan', { explicitId: templateId, agentModel })
  const current = doc.body.trim()
  const input: PlanDocInput = {
    current,
    transcript,
    ...(template ? { templatePrompt: templatePrompt(template, 'the plan document') } : {}),
    ...(await routingContext()
      .then((map) => (map ? { routingMap: map } : {}))
      .catch(() => ({}))),
  }

  // The plan's OWN agent writes the document, so the model is pinned rather than
  // resolved from a chain. `tier` is named separately from the base agent
  // because the runner needs both: it calls `<agent>-<alias>` and meters the
  // spend against `<agent>`. Nothing here supplies a transport any more — the
  // runner routes a persona tier itself and carries this attribution on the
  // request (see `planTier` above).
  const tier = planTier(agentModel, routedModel)
  const result = await runHarness(planDocHarness, input, {
    caller: `plan:${conversationId}`,
    model: agentModel,
    ...(tier ? { tier } : {}),
    ledger: { source: 'chat', refId: conversationId },
  })

  const body = result.value
  if (!body) {
    // The runner reports a failure in harness terms, which is the right sentence
    // for `harness_runs` and the wrong one for a toast on the Plan surface. A
    // reply that arrived and carried nothing keeps the wording this route has
    // always thrown; anything else means we never got an answer, and the
    // runner's sentence names why. `HarnessResult.answered` is that fact under
    // its own name — this derived it from `model !== null && raw !== null`, and
    // `raw` is a bounded drill-down field, not a control-flow signal.
    throw new Error(result.answered ? 'the agent returned an empty document' : (result.error ?? 'the agent could not be reached'))
  }
  // THE DATA-LOSS GUARD (see harness/defs/plan-doc.ts). The reply is a whole
  // document and it is about to replace one, so a rewrite that lost most of its
  // sections, or dropped sections while coming back shorter, or kept the
  // headings and threw away the substance, is not saved at all. The document the
  // plan already has is always the better answer to "that reply was damage".
  const regression = planDocRegression(current, body)
  if (regression) throw new Error(`the agent returned ${regression}; the existing document was kept`)

  const saved = (await saveArtifact(doc.id, { body }, label)) ?? doc
  void indexPlanDoc(saved, conversationId).catch(() => {})
  return saved
}

/** Notify teammates a plan message @mentions — only members who can actually
 *  read the plan's document (owner-private plans mention silently until the
 *  doc is shared). Before the doc exists, the plan's own membership is the
 *  read boundary. Fire-and-forget friendly. */
export async function notifyPlanMentions(
  conversationId: string,
  sender: { id: string; label: string },
  content: string,
  planTitle: string | null,
): Promise<void> {
  if (!content.includes('@')) return
  const doc = await planDocFor(conversationId)
  let eligible: Array<{ userId: string; name: string | null; email: string | null }>
  if (doc) {
    const grants = await listEditors('artifact', doc.id)
    eligible = (await listUsers())
      .filter((u) => canRead(guarded(doc), u.id, u.email ?? u.name, grants))
      .map((u) => ({ userId: u.id, name: u.name, email: u.email }))
  } else {
    eligible = await listPlanMembers(conversationId)
  }
  await notifyMentions(
    eligible,
    sender.id,
    sender.label,
    content,
    `a plan (${planTitle || 'Untitled'})`,
    `/plan?p=${conversationId}`,
  )
}
