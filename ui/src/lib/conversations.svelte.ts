import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { delJson, errorMessage, getJson, getList, postJson, putJson } from '@/lib/fetch-json'
import { pushToast } from '@/lib/toast.svelte'
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

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

/** Plan membership + live presence. Pings presence while mounted and polls so
 *  everyone's avatars/dots stay current. */
export function usePlanMembers(planId: MaybeGetter<string | null>) {
  const qc = useQueryClient()
  $effect(() => {
    const id = resolve(planId)
    if (!id) return
    // Best-effort presence ping — a dropped one just means a stale dot until
    // the next tick, so a failure is swallowed rather than surfaced.
    const ping = () => putJson<{ ok: true }>(`/api/plans/${id}/members`).catch(() => {})
    void ping()
    const t = setInterval(() => {
      void ping()
      void qc.invalidateQueries({ queryKey: ['plan-members', id] })
    }, 25_000)
    return () => clearInterval(t)
  })
  return createQuery(() => {
    const id = resolve(planId)
    return {
      queryKey: ['plan-members', id],
      enabled: !!id,
      queryFn: (): Promise<{ members: PlanMember[]; active: string[] }> =>
        getJson<{ members: PlanMember[]; active: string[] }>(`/api/plans/${id}/members`),
    }
  })
}

export const sharePlan = async (planId: string, email: string): Promise<void> => {
  await postJson<{ members: PlanMember[] }>(`/api/plans/${planId}/members`, { email })
}

export const unsharePlan = async (planId: string, userId: string): Promise<void> => {
  // The call site fires and forgets (`.then(refresh)`, no catch), so a refused
  // remove is surfaced here rather than left as an unhandled rejection.
  await delJson<{ members: PlanMember[] }>(`/api/plans/${planId}/members`, { userId }).catch((e: unknown) =>
    pushToast({ title: 'Remove failed', body: errorMessage(e), tone: 'danger' }),
  )
}

export function useConversations(kind: MaybeGetter<'chat' | 'plan'> = 'chat') {
  return createQuery(() => {
    const k = resolve(kind)
    return {
      queryKey: ['conversations', k],
      queryFn: (): Promise<Conversation[]> => getList<Conversation>(`/api/conversations?kind=${k}`, 'conversations'),
      // While any thread has a reply in flight, keep the list fresh so the
      // "working" indicator (and the reply's completion) show up on their own.
      refetchInterval: (q: { state: { data?: Conversation[] } }) =>
        q.state.data?.some((c) => c.working) ? 4_000 : false,
    }
  })
}

// NOT a query function — chat-view drives it imperatively from effects and
// polls, none of which have a rejection handler. Left non-throwing on purpose:
// making it reject here buys unhandled rejections, not an error state. Fixing
// it properly means giving chat-view a load-failure branch (audit follow-up).
export async function loadConversation(
  id: string,
): Promise<{ conversation: Conversation; messages: StoredMessage[] } | null> {
  // Every failure — status or network — reads as "nothing to show" (see the
  // comment above for why this must not reject).
  return getJson<{ conversation: Conversation; messages: StoredMessage[] }>(`/api/conversations/${id}`).catch(
    () => null,
  )
}
