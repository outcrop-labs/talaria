import { parseAgentStream } from '@/lib/sse-parse'
export type { ChatEvent, ToolCall } from '@/lib/sse-parse'

export interface ChatMeta {
  conversationId: string
  messageId: string
}

/**
 * Start a durable streaming turn. The server owns history — we send only the
 * new user message (+ optional conversationId). `onMeta` fires with the
 * conversation/message ids (from response headers) before the stream begins.
 */
export async function* streamChat(
  params: { model: string; conversationId?: string; content: string; tier?: string; attachmentIds?: string[]; kind?: 'chat' | 'plan' },
  onMeta?: (m: ChatMeta) => void,
  signal?: AbortSignal,
) {
  const res = await fetch('/api/chat', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify(params),
    signal,
  })
  if (!res.ok || !res.body) throw new Error(`chat failed: ${res.status}`)

  const conversationId = res.headers.get('X-Conversation-Id')
  const messageId = res.headers.get('X-Message-Id')
  if (conversationId && messageId) onMeta?.({ conversationId, messageId })

  yield* parseAgentStream(res.body)
}
