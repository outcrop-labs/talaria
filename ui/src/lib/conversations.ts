import { useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { getJson, getList } from '@/lib/fetch-json'
import type { ToolCall } from '@/lib/sse-parse'

export interface Conversation {
  id: string
  agentModel: string
  title: string | null
  updatedAt: string
  /** An assistant reply is streaming right now (the agent is working). */
  working?: boolean
  /** The latest assistant turn errored — needs a re-run. */
  failed?: boolean
  /** Multiplayer plans: your standing; ownerLabel set on plans shared with you. */
  role?: 'owner' | 'collaborator'
  ownerLabel?: string | null
}

export interface StoredMessage {
  role: 'user' | 'assistant'
  content: string
  reasoning: string
  tools: ToolCall[]
  status: 'streaming' | 'complete' | 'error'
  seq: number
  attachments?: Array<{ id: string; filename: string; mime: string; size: number }>
  /** Who wrote a user turn (multiplayer plans). */
  authorUserId?: string | null
  authorLabel?: string | null
  /** Confab-guard findings pinned to a reply (annotate/strict modes). */
  guard?: Array<{ check: string; severity: 'low' | 'medium' | 'high'; confidence: number; message: string; snippet: string }> | null
}

export interface PlanMember {
  userId: string
  name: string | null
  email: string | null
  role: 'owner' | 'collaborator'
}

/** Plan membership + live presence. Pings presence while mounted and polls so
 *  everyone's avatars/dots stay current. */
export function usePlanMembers(planId: string | null) {
  const qc = useQueryClient()
  useEffect(() => {
    if (!planId) return
    const ping = () =>
      fetch(`/api/plans/${planId}/members`, { method: 'PUT', credentials: 'same-origin' }).catch(() => {})
    void ping()
    const t = setInterval(() => {
      void ping()
      void qc.invalidateQueries({ queryKey: ['plan-members', planId] })
    }, 25_000)
    return () => clearInterval(t)
  }, [planId, qc])
  return useQuery({
    queryKey: ['plan-members', planId],
    enabled: !!planId,
    queryFn: (): Promise<{ members: PlanMember[]; active: string[] }> =>
      getJson<{ members: PlanMember[]; active: string[] }>(`/api/plans/${planId}/members`),
  })
}

export const sharePlan = async (planId: string, email: string): Promise<void> => {
  const r = await fetch(`/api/plans/${planId}/members`, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ email }),
  })
  if (!r.ok) throw new Error(((await r.json().catch(() => ({}))) as { error?: string }).error ?? 'share failed')
}

export const unsharePlan = async (planId: string, userId: string): Promise<void> => {
  await fetch(`/api/plans/${planId}/members`, {
    method: 'DELETE',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ userId }),
  })
}

export function useConversations(kind: 'chat' | 'plan' = 'chat') {
  return useQuery({
    queryKey: ['conversations', kind],
    queryFn: (): Promise<Conversation[]> => getList<Conversation>(`/api/conversations?kind=${kind}`, 'conversations'),
    // While any thread has a reply in flight, keep the list fresh so the
    // "working" indicator (and the reply's completion) show up on their own.
    refetchInterval: (q) => (q.state.data?.some((c) => c.working) ? 4_000 : false),
  })
}

// NOT a useQuery function — chat-view drives it imperatively from effects and
// polls, none of which have a rejection handler. Left non-throwing on purpose:
// making it reject here buys unhandled rejections, not an error state. Fixing
// it properly means giving chat-view a load-failure branch (audit follow-up).
export async function loadConversation(
  id: string,
): Promise<{ conversation: Conversation; messages: StoredMessage[] } | null> {
  const r = await fetch(`/api/conversations/${id}`, { credentials: 'same-origin' })
  if (!r.ok) return null
  return r.json()
}
