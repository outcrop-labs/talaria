// Server-side persistence of an assistant stream. Runs detached from the client
// response (fed by a teed branch), so an in-progress reply is saved even if the
// client disconnects/reloads. Throttled flushes during streaming; final on end.
// Also records the turn in the token ledger (real usage or a char estimate).

import { parseAgentStream, mergeTool, type ToolCall } from '@/lib/sse-parse'
import { touchConversation, updateAssistant } from './conversations'
import { estimateTokens, recordUsage } from './usage'

export async function persistAssistantStream(
  stream: ReadableStream<Uint8Array>,
  messageId: string,
  conversationId: string,
  usageMeta?: { agentModel: string; promptChars: number; tier?: string | null },
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
  } catch {
    await flush('error').catch(() => {})
    ledger()
  }
}
