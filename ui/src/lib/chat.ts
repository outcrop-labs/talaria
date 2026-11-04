import { postJson, postStream } from '@/lib/fetch-json'
import { parseAgentStream } from '@/lib/sse-parse'
export type { ChatEvent, ToolCall } from '@/lib/sse-parse'

export interface ChatMeta {
  conversationId: string
  messageId: string
}

/** Which surface a turn belongs to — the discriminator on `conversations`,
 *  and the one the /api/chat route validates against. Named once because
 *  three signatures and a component prop all take it; spelled out in each,
 *  adding a surface meant finding all four. Must stay in lockstep with the
 *  server's whitelist (talaria-routes-comms `chat.rs`). */
export type ChatKind = 'chat' | 'plan' | 'research' | 'ticket' | 'work'

/** THE FILE OPEN IN THE TEAMMATE'S PANE, sent with a work turn so the agent
 *  knows what "this doc" means. The server validates it (an id of at most 200
 *  `[A-Za-z0-9_-]`, a title of at most 300 units with control characters
 *  stripped) and quotes it into one system line for `work` turns only; any
 *  other kind ignores it. */
export interface ChatFocus {
  kind: 'artifact' | 'google'
  id: string
  title?: string
}

/**
 * Start a durable streaming turn. The server owns history — we send only the
 * new user message (+ optional conversationId). `onMeta` fires with the
 * conversation/message ids (from response headers) before the stream begins.
 * `turnId` is the caller's own name for this turn (a uuid it draws), which a
 * Stop can carry before the server has heard of the send (`stopChat`,
 * `stopTurn`).
 */
export async function* streamChat(
  params: { model: string; conversationId?: string; content: string; tier?: string; effort?: string; attachmentIds?: string[]; refs?: Array<{ type: 'kb-doc' | 'artifact'; id: string }>; kind?: ChatKind; templateId?: string | null; focus?: ChatFocus; turnId?: string },
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
  kind?: ChatKind
  focus?: ChatFocus
}): Promise<void> {
  // The route answers 202 `{ queued: true, conversationId }` — parsed only to
  // prove it arrived; the caller has nothing to read from it.
  await postJson<{ queued: true; conversationId: string }>('/api/chat', { ...params, queue: true })
}

/** What a stop did. `hermes` says the agent's RUN was told to stop (by its run
 *  id on a session turn, by its completion id on a completions turn), not
 *  merely this reader's stream. `held` says the agent had not opened the turn
 *  within the route's wait, and the server sends the stop when it does.
 *  Callers fire and forget; the fields are named because the route answers
 *  them and the wire is worth reading in a devtools log. */
export interface StopResult {
  ok: true
  hermes: boolean
  held?: true
}

/** A stop outlives the page that sent it: pressed and then closed, the
 *  request still goes out (`keepalive`), and the server holds what it cannot
 *  place yet. */
const STOP_INIT: RequestInit = { keepalive: true }

/** Stop the turn a conversation is streaming. A reader's abort is LOCAL — it
 *  ends this client's stream and nothing else — so a Hermes run carried on
 *  generating (and spending) behind a frozen transcript. This is the half that
 *  reaches the run. `turnId` names this page's own send, so a stop that
 *  reaches the server before that send has is held for it rather than lost.
 *  Nothing streaming is a 409 and throws like any other non-2xx: a stop that
 *  arrives after the turn landed has nothing to do. */
export async function stopChat(conversationId: string, turnId?: string): Promise<StopResult> {
  return postJson<StopResult>(
    `/api/conversations/${encodeURIComponent(conversationId)}/stop`,
    turnId ? { turnId } : undefined,
    STOP_INIT,
  )
}

/** Stop this page's own send by the name it gave it (`streamChat`'s
 *  `turnId`): the stop a new thread's first turn needs, since its thread id
 *  arrives only with the response headers. The server waits for that send,
 *  or holds the stop for it. */
export async function stopTurn(turnId: string): Promise<StopResult> {
  return postJson<StopResult>(`/api/chat/turns/${encodeURIComponent(turnId)}/stop`, undefined, STOP_INIT)
}

/** Where a mid-turn message landed. `steered` — the running turn took it;
 *  `queued` — its window had closed, so the agent picks it up after this
 *  reply, exactly as `queueChatMessage` would have. Either way the server owns
 *  the row it wrote, and `messageId` is that row. */
export type SteerResult = { steered: true; messageId: string } | { queued: true; messageId: string }

/** Send NOW: hand a message to the reply that is already streaming, instead of
 *  queueing it behind that reply (`queueChatMessage`, above). Which of the two
 *  actually happened is the server's call — a run past its steering window
 *  queues — so the caller reads the answer rather than assuming it. A 409
 *  means nothing is streaming any more, and the ordinary send path is then
 *  exactly right. */
export async function steerChat(params: { conversationId: string; content: string }): Promise<SteerResult> {
  return postJson<SteerResult>(`/api/conversations/${encodeURIComponent(params.conversationId)}/steer`, {
    content: params.content,
  })
}
