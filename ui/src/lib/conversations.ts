import { useQuery } from '@tanstack/react-query'
import type { ToolCall } from '@/lib/sse-parse'

export interface Conversation {
  id: string
  agentModel: string
  title: string | null
  updatedAt: string
}

export interface StoredMessage {
  role: 'user' | 'assistant'
  content: string
  reasoning: string
  tools: ToolCall[]
  status: 'streaming' | 'complete' | 'error'
  seq: number
  attachments?: Array<{ id: string; filename: string; mime: string; size: number }>
}

export function useConversations() {
  return useQuery({
    queryKey: ['conversations'],
    queryFn: async (): Promise<Conversation[]> => {
      const r = await fetch('/api/conversations', { credentials: 'same-origin' })
      if (!r.ok) return []
      const data = (await r.json()) as { conversations: Conversation[] }
      return data.conversations
    },
  })
}

export async function loadConversation(
  id: string,
): Promise<{ conversation: Conversation; messages: StoredMessage[] } | null> {
  const r = await fetch(`/api/conversations/${id}`, { credentials: 'same-origin' })
  if (!r.ok) return null
  return r.json()
}
