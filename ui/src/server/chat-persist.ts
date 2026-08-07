// Server-side persistence of an assistant stream. Runs detached from the client
// response (fed by a teed branch), so an in-progress reply is saved even if the
// client disconnects/reloads. Throttled flushes during streaming; final on end.
// Also records the turn in the token ledger (real usage or a char estimate).

import { parseAgentStream, mergeTool, type ToolCall } from '@/lib/sse-parse'
import { maybeRetitleConversation } from './titler'
import {
  activeStreamingAssistant,
  insertStreamingAssistant,
  nextSeq,
  priorMessages,
  setMessageGuard,
  touchConversation,
  updateAssistant,
} from './conversations'
import { routedModelFor } from './fleet-agents'
import { estimateTokens, recordUsage } from './usage'
import { guardChatReply, needsRedaction, redactFindings, redactSecrets } from './guardrails'
import { notifyPlanMentions, PLAN_MODE_PROMPT, planRoutingBlock } from './plan-doc'
import { describeAgent, proxyChat } from './gateway'
import { indexActivity } from './retrieval/sources'

export interface TurnMeta {
  agentModel: string
  tier?: string | null
  /** Set for plan conversations: replies feed the activity brain, owner-scoped. */
  plan?: { ownerUserId: string; title: string | null } | null
}

// One continuation at a time per conversation (in-process guard — the check
// below re-reads the DB, this just closes the tiny double-start window).
const continuing = new Set<string>()

/** Claude-style flow: messages sent while a reply streamed queued into
 *  history — start the NEXT turn covering them. Called when a reply finishes
 *  and when a queued message lands with nothing in flight; chains until the
 *  conversation goes quiet. No-op unless the last message is the user's. */
export async function continueConversation(conversationId: string, meta: TurnMeta): Promise<void> {
  if (continuing.has(conversationId)) return
  continuing.add(conversationId)
  try {
    if (await activeStreamingAssistant(conversationId)) return
    const prior = await priorMessages(conversationId)
    if (prior[prior.length - 1]?.role !== 'user') return
    // Chained plan turns carry the same plan-mode harness as live ones.
    const messages = meta.plan ? [{ role: 'system' as const, content: PLAN_MODE_PROMPT + (await planRoutingBlock().catch(() => '')) }, ...prior] : prior
    const routed = (meta.tier ? await routedModelFor(meta.agentModel, meta.tier).catch(() => null) : null) ?? meta.agentModel
    const assistantId = await insertStreamingAssistant(conversationId, await nextSeq(conversationId))
    const upstream = await proxyChat({ model: routed, messages })
    if (!upstream.ok || !upstream.body) {
      await updateAssistant(assistantId, { content: '', reasoning: '', tools: [], status: 'error' })
      return
    }
    const promptChars = messages.reduce((n, m) => n + m.content.length, 0)
    void persistAssistantStream(upstream.body, assistantId, conversationId, {
      agentModel: meta.agentModel,
      promptChars,
      tier: meta.tier ?? null,
      plan: meta.plan ?? null,
    })
  } finally {
    continuing.delete(conversationId)
  }
}

export async function persistAssistantStream(
  stream: ReadableStream<Uint8Array>,
  messageId: string,
  conversationId: string,
  usageMeta?: {
    agentModel: string
    promptChars: number
    tier?: string | null
    /** Set for plan conversations: the reply feeds the activity brain, owner-scoped. */
    plan?: { ownerUserId: string; title: string | null } | null
  },
): Promise<void> {
  let content = ''
  let reasoning = ''
  let tools: ToolCall[] = []
  let usage: { promptTokens: number; completionTokens: number } | null = null
  let lastFlush = 0

  const flush = (status: 'streaming' | 'complete' | 'error') =>
    updateAssistant(messageId, { content, reasoning, tools, status })

  const ledger = () => {
    if (!usageMeta) return
    void recordUsage({
      agentModel: usageMeta.agentModel,
      source: 'chat',
      refId: conversationId,
      tier: usageMeta.tier ?? null,
      promptTokens: usage?.promptTokens ?? estimateTokens(usageMeta.promptChars),
      completionTokens: usage?.completionTokens ?? estimateTokens(content.length + reasoning.length),
      estimated: !usage,
    }).catch(() => {})
  }

  try {
    for await (const ev of parseAgentStream(stream)) {
      if (ev.type === 'content') content += ev.text
      else if (ev.type === 'reasoning') reasoning += ev.text
      else if (ev.type === 'tool') tools = mergeTool(tools, ev)
      else if (ev.type === 'usage') usage = ev

      const now = Date.now()
      if (now - lastFlush > 400) {
        lastFlush = now
        await flush('streaming')
      }
    }
    await flush('complete')
    await touchConversation(conversationId)
    ledger()
    // First-exchange naming: the Titler upgrades the mechanical truncated
    // title once there's a real exchange to name (early-outs make this a
    // single cheap query on every later reply).
    void maybeRetitleConversation(conversationId).catch(() => {})
    // Confab guard on the final reply (structural). The fleet stream gives tool
    // names, so zero-tool-claim + secret-leak apply here. annotate/strict pin
    // the findings onto the message row (the UI renders a caveat; transcripts
    // never see it); strict also redacts leaked secrets from the SAVED copy so
    // future turns can't re-feed them.
    //
    // AWAITED, AND AHEAD OF THE INDEX AND THE NOTIFICATION, because both take a
    // COPY of `content`. As a detached IIFE below them, strict mode scrubbed the
    // `messages` row while `indexActivity` had already put the unredacted reply
    // into the owner's brain — where nothing ever re-indexes it, and where
    // `search_knowledge` hands it back to a model, which is the one thing
    // guardrails.ts's cardinal invariant forbids — and `notifyPlanMentions` had
    // already mailed it. channels.$id.messages.ts and tasks.$id.comments.ts
    // order these the same way, for the same reason. Failure here must not fail
    // the persist, so the whole block is caught rather than the outer `catch`
    // flushing the message as errored.
    if (usageMeta && content) {
      await (async () => {
        const { findings, mode } = await guardChatReply({
          answer: content,
          toolNames: tools.map((t) => t.name),
          userMessage: '',
          caller: `chat:${usageMeta.agentModel}`,
          model: usageMeta.agentModel,
        })
        if (!findings.length || (mode !== 'annotate' && mode !== 'strict')) return
        if (mode === 'strict' && needsRedaction(findings)) {
          content = redactSecrets(content).text
          reasoning = redactSecrets(reasoning).text
          await flush('complete')
        }
        // Scrubbed: a pinned finding carries a verbatim excerpt of the flagged
        // span, and `zero_tool_claim` does not truncate its own.
        await setMessageGuard(messageId, redactFindings(findings))
      })().catch(() => {})
    }
    if (usageMeta?.plan && content.trim()) {
      void indexActivity({
        sourceType: 'plan',
        sourceId: messageId,
        title: `Plan (${usageMeta.plan.title || 'Untitled'}) · ${describeAgent(usageMeta.agentModel).label}`,
        text: content,
        payload: { planId: conversationId, planOwnerId: usageMeta.plan.ownerUserId },
        href: '/plan',
      }).catch(() => {})
      // An agent turn @mentioning a collaborator notifies like a human one.
      void notifyPlanMentions(
        conversationId,
        { id: '', label: describeAgent(usageMeta.agentModel).label },
        content,
        usageMeta.plan.title,
      ).catch(() => {})
    }
    // Messages queued while this reply streamed become the next turn.
    if (usageMeta) {
      void continueConversation(conversationId, {
        agentModel: usageMeta.agentModel,
        tier: usageMeta.tier ?? null,
        plan: usageMeta.plan ?? null,
      }).catch(() => {})
    }
  } catch {
    await flush('error').catch(() => {})
    ledger()
  }
}
