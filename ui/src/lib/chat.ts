import { postJson, postStream } from '@/lib/fetch-json'
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
  params: { model: string; conversationId?: string; content: string; tier?: string; effort?: string; attachmentIds?: string[]; refs?: Array<{ type: 'kb-doc' | 'artifact'; id: string }>; kind?: 'chat' | 'plan' | 'research'; templateId?: string | null },
  onMeta?: (m: ChatMeta) => void,
  signal?: AbortSignal,
) {
  const res = await postStream('/api/chat', params, { signal })
  // A JSON reply is "queued" (a reply was already streaming server-side; the
  // message joined history for the next turn) — errors already threw in the door.
  if (res.headers.get('content-type')?.includes('application/json')) {
    const j = (await res.json().catch(() => ({}))) as { queued?: boolean; conversationId?: string }
    if (j.queued && j.conversationId) {
      onMeta?.({ conversationId: j.conversationId, messageId: '' })
      yield { type: 'queued' } as const
      return
    }
    throw new Error('unexpected chat response')
  }

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
  effort?: string
  attachmentIds?: string[]
  refs?: Array<{ type: 'kb-doc' | 'artifact'; id: string }>
  kind?: 'chat' | 'plan' | 'research'
}): Promise<void> {
  // The route answers 202 `{ queued: true, conversationId }` — parsed only to
  // prove it arrived; the caller has nothing to read from it.
  await postJson<{ queued: true; conversationId: string }>('/api/chat', { ...params, queue: true })
}
