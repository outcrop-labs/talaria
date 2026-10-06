// The sidebar's two cross-conversation reads (KTD10): the threads you are in,
// and what you recently sent. Drafts are local — see comms-drafts.ts.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'
import type { ChannelKind, ChannelMessage } from '@/lib/channels.svelte'
import type { ProfileSubject, SharedConversation } from '@/lib/comms-profile'

export interface MyThread {
  channelId: string
  channelName: string
  channelKind: ChannelKind
  root: ChannelMessage
  /** The latest reply's time — the list's order, newest first. */
  lastAt: string
}

export interface SentMessage {
  kind: 'channel' | 'agent'
  /** The channel id, or the agent conversation id. */
  conversationId: string
  title: string | null
  content: string
  createdAt: string
}

export function useMyThreads() {
  return createQuery(() => ({
    queryKey: ['me', 'threads'],
    queryFn: async (): Promise<MyThread[]> => (await getJson<{ threads: MyThread[] }>('/api/me/threads')).threads,
    staleTime: 15_000,
  }))
}

export function useMySent() {
  return createQuery(() => ({
    queryKey: ['me', 'sent'],
    queryFn: async (): Promise<SentMessage[]> => (await getJson<{ messages: SentMessage[] }>('/api/me/sent')).messages,
    staleTime: 15_000,
  }))
}

/** Every conversation the viewer shares with one person or agent, newest
 *  activity first — the profile drawer's list and the /comms/with view. */
export function useConversationsWith(subject: () => ProfileSubject | null) {
  return createQuery(() => {
    const s = subject()
    const path = !s
      ? null
      : s.kind === 'person'
        ? `/api/users/${encodeURIComponent(s.userId)}/conversations`
        : `/api/agents/${encodeURIComponent(s.model)}/conversations`
    return {
      queryKey: ['conversations-with', s?.kind ?? null, s?.kind === 'person' ? s.userId : (s?.model ?? null)],
      queryFn: async (): Promise<SharedConversation[]> =>
        (await getJson<{ conversations: SharedConversation[] }>(path!)).conversations,
      enabled: !!path,
      staleTime: 15_000,
    }
  })
}
