// Queue anchoring for <ChatView> — the rules that keep a streaming turn alive
// while user messages queue behind it (the bug this module exists to pin:
// everything used to anchor to messages.length - 1, so a queued user row
// turned the live transcript dark and dropped stream events).
import type { DisplayMessage } from './chat-view'

/** Index of the LAST assistant row, or -1 when the thread has none. Stream
 *  events and the live indicator anchor here — the assistant turn in flight —
 *  never to the absolute end of the thread, where queued user rows land. */
export const lastAssistantIndex = (messages: DisplayMessage[]): number => {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i]?.role === 'assistant') return i
  }
  return -1
}

/** How many user messages are queued BEHIND the turn in flight: the user rows
 *  after the last assistant row. Non-empty only when the newest assistant turn
 *  is still unresolved (streaming on our side, or a persisted server-owned row
 *  still in flight) — once every turn has resolved, nothing is waiting. */
export const pendingCount = (messages: DisplayMessage[]): number => {
  let i = messages.length - 1
  while (i >= 0 && messages[i]?.role === 'user') i--
  const head = i >= 0 ? messages[i] : undefined
  if (!head || head.role !== 'assistant' || head.status !== 'streaming') return 0
  return messages.length - 1 - i
}
