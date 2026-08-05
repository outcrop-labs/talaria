// Shared message shape for <ChatView> and its turn rows (see ChatView.svelte).
import type { ToolCall } from '@/lib/sse-parse'
import type { StoredMessage } from '@/lib/conversations.svelte'
import type { Attachment } from '@/lib/attachments'
import type { GuardFinding } from '@/components/chat/guard-caveat'

export interface DisplayMessage {
  role: 'user' | 'assistant'
  content: string
  reasoning?: string
  tools?: ToolCall[]
  status?: 'streaming' | 'complete' | 'error'
  attachments?: Attachment[]
  /** Who wrote a user turn — shown in multiplayer plans to tell voices apart. */
  authorLabel?: string | null
  /** Confab-guard findings pinned to a reply (annotate/strict modes). */
  guard?: GuardFinding[] | null
}

export const toDisplay = (m: StoredMessage): DisplayMessage => ({
  role: m.role,
  content: m.content,
  reasoning: m.reasoning,
  tools: m.tools,
  status: m.status,
  attachments: m.attachments,
  authorLabel: m.authorLabel,
  guard: m.guard,
})
