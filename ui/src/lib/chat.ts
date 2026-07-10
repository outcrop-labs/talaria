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
  params: { model: string; conversationId?: string; content: string; tier?: string; attachmentIds?: string[]; refs?: Array<{ type: 'kb-doc' | 'artifact'; id: string }>; kind?: 'chat' | 'plan' },
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
  // A JSON reply is either an error or "queued" (a reply was already
  // streaming server-side; the message joined history for the next turn).
  if (res.headers.get('content-type')?.includes('application/json')) {
    const j = (await res.json().catch(() => ({}))) as { queued?: boolean; conversationId?: string; error?: string }
    if (!res.ok || j.error) throw new Error(j.error ?? `chat failed: ${res.status}`)
    if (j.queued && j.conversationId) {
      onMeta?.({ conversationId: j.conversationId, messageId: '' })
      yield { type: 'queued' } as const
      return
    }
    throw new Error('unexpected chat response')
  }
  if (!res.ok || !res.body) throw new Error(`chat failed: ${res.status}`)

  const conversationId = res.headers.get('X-Conversation-Id')
  const messageId = res.headers.get('X-Message-Id')
  if (conversationId && messageId) onMeta?.({ conversationId, messageId })

  yield* parseAgentStream(res.body)
}

/** Send while a reply is streaming: the server queues the message into
 *  history (never interrupting) and chains the next turn when the current
 *  reply finishes. */
export async function queueChatMessage(params: {
  model: string
  conversationId: string
  content: string
  tier?: string
  attachmentIds?: string[]
  refs?: Array<{ type: 'kb-doc' | 'artifact'; id: string }>
  kind?: 'chat' | 'plan'
}): Promise<void> {
  const res = await fetch('/api/chat', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ ...params, queue: true }),
  })
  if (!res.ok) {
    const j = (await res.json().catch(() => ({}))) as { error?: string }
    throw new Error(j.error ?? `send failed (${res.status})`)
  }
}
