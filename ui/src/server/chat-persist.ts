// Server-side persistence of an assistant stream. Runs detached from the client
// response (fed by a teed branch), so an in-progress reply is saved even if the
// client disconnects/reloads. Throttled flushes during streaming; final on end.
// Also records the turn in the token ledger (real usage or a char estimate).

import { parseAgentStream, mergeTool, type ToolCall } from '@/lib/sse-parse'
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
import { guardChatReply, redactSecrets } from './guardrails'
import { PLAN_MODE_PROMPT } from './plan-doc'
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
    const messages = meta.plan ? [{ role: 'system' as const, content: PLAN_MODE_PROMPT }, ...prior] : prior
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
    if (usageMeta?.plan && content.trim()) {
      void indexActivity({
        sourceType: 'plan',
        sourceId: messageId,
        title: `Plan (${usageMeta.plan.title || 'Untitled'}) · ${describeAgent(usageMeta.agentModel).label}`,
        text: content,
        payload: { planId: conversationId, planOwnerId: usageMeta.plan.ownerUserId },
        href: '/plan',
      }).catch(() => {})
    }
    // Messages queued while this reply streamed become the next turn.
    if (usageMeta) {
      void continueConversation(conversationId, {
        agentModel: usageMeta.agentModel,
        tier: usageMeta.tier ?? null,
        plan: usageMeta.plan ?? null,
      }).catch(() => {})
    }
    // Confab guard on the final reply (structural; fire-and-forget). The fleet
    // stream gives tool names, so zero-tool-claim + secret-leak apply here.
    // annotate/strict pin the findings onto the message row (the UI renders a
    // caveat; transcripts never see it); strict also redacts leaked secrets
    // from the SAVED copy so future turns can't re-feed them.
    if (usageMeta && content) {
      void (async () => {
        const { findings, mode } = await guardChatReply({
          answer: content,
          toolNames: tools.map((t) => t.name),
          userMessage: '',
          caller: `chat:${usageMeta.agentModel}`,
          model: usageMeta.agentModel,
        })
        if (!findings.length || (mode !== 'annotate' && mode !== 'strict')) return
        if (mode === 'strict' && findings.some((f) => f.check === 'secret_leak')) {
          content = redactSecrets(content).text
          reasoning = redactSecrets(reasoning).text
          await flush('complete')
        }
        await setMessageGuard(messageId, findings)
      })().catch(() => {})
    }
  } catch {
    await flush('error').catch(() => {})
    ledger()
  }
}
