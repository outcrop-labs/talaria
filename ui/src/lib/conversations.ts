import { useQuery } from '@tanstack/react-query'
import type { ToolCall } from '@/lib/sse-parse'

export interface Conversation {
  id: string
  agentModel: string
  title: string | null
  updatedAt: string
  /** An assistant reply is streaming right now (the agent is working). */
  working?: boolean
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

export function useConversations(kind: 'chat' | 'plan' = 'chat') {
  return useQuery({
    queryKey: ['conversations', kind],
    queryFn: async (): Promise<Conversation[]> => {
      const r = await fetch(`/api/conversations?kind=${kind}`, { credentials: 'same-origin' })
      if (!r.ok) return []
      const data = (await r.json()) as { conversations: Conversation[] }
      return data.conversations
    },
    // While any thread has a reply in flight, keep the list fresh so the
    // "working" indicator (and the reply's completion) show up on their own.
    refetchInterval: (q) => (q.state.data?.some((c) => c.working) ? 4_000 : false),
  })
}

export async function loadConversation(
  id: string,
): Promise<{ conversation: Conversation; messages: StoredMessage[] } | null> {
  const r = await fetch(`/api/conversations/${id}`, { credentials: 'same-origin' })
  if (!r.ok) return null
  return r.json()
}
