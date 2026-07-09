// The plan's living document, server-side. The document IS a `doc` artifact
// linked to the plan conversation (artifact_links target_type='plan') — no
// separate model. This module finds/creates that artifact, lets the plan's own
// agent rewrite it from the conversation, keeps it in the activity index, and
// notifies teammates the plan @mentions (only ones who can read the document).
import { parseAgentStream } from '@/lib/sse-parse'
import {
  artifactsForTarget,
  attachArtifact,
  createArtifact,
  guarded,
  saveArtifact,
  type Artifact,
} from './artifacts'
import { priorMessages } from './conversations'
import { describeAgent, proxyChat } from './gateway'
import { canRead, listEditors } from './kb-perms'
import { notifyMentions } from './mentions'
import { indexActivity } from './retrieval/sources'
import { estimateTokens, recordUsage } from './usage'
import { listUsers } from './users'

const SYNC_PROMPT = `You maintain the living plan document for a planning conversation. Rewrite the document so it reflects the conversation so far: goals, scope, decisions, open questions, and next steps — organized under markdown headings, tight and actionable.
Start from the current version when one is given: keep what still holds, fold in what changed, never silently drop sections the conversation didn't overturn.
Return ONLY the complete updated markdown document, starting with its "# " title heading as your very first characters — no commentary, no lead-in sentence, no code fences. Anything before the first heading corrupts the document.`

/** The plan's linked doc artifact, if one exists yet. */
export async function planDocFor(conversationId: string): Promise<Artifact | null> {
  const linked = await artifactsForTarget('plan', conversationId)
  return linked.find((a) => a.kind === 'doc') ?? null
}

/** Find-or-create the plan's document (the UI normally creates it on first
 *  open; this covers syncs that arrive first). Owned by the plan's owner. */
export async function ensurePlanDoc(
  conversationId: string,
  owner: { id: string; label: string },
  planTitle: string | null,
): Promise<Artifact> {
  const existing = await planDocFor(conversationId)
  if (existing) return existing
  const artifact = await createArtifact({
    kind: 'doc',
    title: `Plan — ${planTitle || 'Untitled'}`,
    createdBy: owner.label,
    ownerUserId: owner.id,
  })
  await attachArtifact(artifact.id, { targetType: 'plan', targetId: conversationId }, owner.label)
  return artifact
}

/** Keep the activity brain current on a plan document (ACL: the plan's owner). */
export async function indexPlanDoc(doc: Artifact, conversationId: string): Promise<void> {
  await indexActivity({
    sourceType: 'plan-doc',
    sourceId: doc.id,
    title: doc.title,
    text: `${doc.title}\n\n${doc.body}`,
    payload: { planId: conversationId, planOwnerId: doc.ownerUserId },
    href: '/artifacts',
  })
}

/** A model reply that wraps the whole document in a fence loses the fence, and
 *  a short conversational lead-in before the first "# " heading is dropped —
 *  persona agents narrate ("I'll update the plan…") despite the prompt. */
const cleanDoc = (s: string): string => {
  let text = s.trim()
  const fenced = /^```[a-z]*\n([\s\S]*)\n```$/.exec(text)
  if (fenced) text = fenced[1]!.trim()
  const heading = text.search(/^# /m)
  if (heading > 0 && heading < 400 && !text.slice(0, heading).includes('#')) text = text.slice(heading)
  return text
}

/** Rewrite the plan document from the conversation, via the plan's own agent
 *  (persona gateway → metered like any chat turn). Returns the saved artifact. */
export async function syncPlanDoc(
  conversationId: string,
  owner: { id: string; label: string },
  planTitle: string | null,
  agentModel: string,
  routedModel: string,
): Promise<Artifact> {
  const doc = await ensurePlanDoc(conversationId, owner, planTitle)
  const label = describeAgent(agentModel).label
  const msgs = await priorMessages(conversationId)
  const transcript = msgs
    .filter((m) => m.content)
    .map((m) => `${m.role === 'assistant' ? label : 'User'}: ${m.content}`)
    .join('\n\n')
  if (!transcript.trim()) return doc

  const current = doc.body.trim()
  const messages = [
    { role: 'system', content: SYNC_PROMPT },
    {
      role: 'user',
      content:
        (current ? `Current document:\n<<<\n${current}\n>>>\n\n` : 'There is no document yet — write one from scratch.\n\n') +
        `Conversation transcript:\n\n${transcript}`,
    },
  ]
  const upstream = await proxyChat({ model: routedModel, messages })
  if (!upstream.ok || !upstream.body) throw new Error(`gateway error ${upstream.status}`)

  let text = ''
  let usage: { promptTokens: number; completionTokens: number } | null = null
  for await (const ev of parseAgentStream(upstream.body)) {
    if (ev.type === 'content') text += ev.text
    else if (ev.type === 'usage') usage = ev
  }
  void recordUsage({
    agentModel,
    source: 'chat',
    refId: conversationId,
    tier: routedModel !== agentModel ? routedModel.slice(agentModel.length + 1) : null,
    promptTokens: usage?.promptTokens ?? estimateTokens(messages.reduce((n, m) => n + m.content.length, 0)),
    completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
    estimated: !usage,
  }).catch(() => {})

  const body = cleanDoc(text)
  if (!body) throw new Error('the agent returned an empty document')
  const saved = (await saveArtifact(doc.id, { body }, label)) ?? doc
  void indexPlanDoc(saved, conversationId).catch(() => {})
  return saved
}

/** Notify teammates a plan message @mentions — only members who can actually
 *  read the plan's document (owner-private plans mention silently until the
 *  doc is shared). Fire-and-forget friendly. */
export async function notifyPlanMentions(
  conversationId: string,
  sender: { id: string; label: string },
  content: string,
  planTitle: string | null,
): Promise<void> {
  if (!content.includes('@')) return
  const doc = await planDocFor(conversationId)
  if (!doc) return
  const grants = await listEditors('artifact', doc.id)
  const readers = (await listUsers()).filter((u) => canRead(guarded(doc), u.id, u.email ?? u.name, grants))
  await notifyMentions(
    readers.map((u) => ({ userId: u.id, name: u.name, email: u.email })),
    sender.id,
    sender.label,
    content,
    `a plan (${planTitle || 'Untitled'})`,
    '/artifacts',
  )
}
