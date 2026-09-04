// Group-chat client: queries + mutations + live SSE refresh.
import { createQuery, useQueryClient } from '@tanstack/svelte-query'
import { delJson, getJson, patchJson, postJson, putJson } from '@/lib/fetch-json'

export type ChannelRole = 'owner' | 'member'

/** 'channel' = persistent + ambient; 'group' = a Relay; 'dm' = human↔human. */
export type ChannelKind = 'channel' | 'group' | 'dm'

export interface Channel {
  id: string
  name: string
  topic: string | null
  kind: ChannelKind
  role: ChannelRole
  createdAt: string
  updatedAt: string
  /** For DMs: the other person. */
  peer?: { userId: string; name: string | null; email: string | null } | null
  /** Others' messages past your read cursor. */
  unreadCount?: number
}

export interface ChannelMember {
  userId: string
  email: string | null
  name: string | null
  role: ChannelRole
}

export interface ChannelDetail {
  role: ChannelRole
  members: ChannelMember[]
  agents: string[]
}

export interface ChannelMessage {
  id: string
  seq: number
  authorType: 'user' | 'agent'
  author: string
  content: string
  status: 'streaming' | 'complete' | 'error'
  createdAt: string
  threadRootId?: string | null
  editedAt?: string | null
  reactions?: Array<{ emoji: string; actors: string[]; actorTypes: string[] }>
  thread?: { count: number; authors: string[]; lastAt: string } | null
  attachments?: Array<{ id: string; filename: string; mime: string; size: number }>
  /** Confab-guard findings pinned to an agent reply (annotate/strict modes). */
  guard?: Array<{ check: string; severity: 'low' | 'medium' | 'high'; confidence: number; message: string; snippet: string }> | null
}

/** A reactive argument: pass a plain value, or a getter for values that change
 *  over a component's life (route params, selections). */
type MaybeGetter<T> = T | (() => T)
const resolve = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export function useChannels() {
  return createQuery(() => ({
    queryKey: ['channels'],
    queryFn: async (): Promise<Channel[]> => (await getJson<{ channels: Channel[] }>('/api/channels')).channels,
  }))
}

export function useChannelDetail(id: MaybeGetter<string | null>) {
  return createQuery(() => {
    const cid = resolve(id)
    return {
      queryKey: ['channel', cid],
      enabled: !!cid,
      queryFn: async (): Promise<ChannelDetail> => getJson<ChannelDetail>(`/api/channels/${cid}`),
    }
  })
}

export function useChannelMessages(id: MaybeGetter<string | null>) {
  return createQuery(() => {
    const cid = resolve(id)
    return {
      queryKey: ['channel-messages', cid],
      enabled: !!cid,
      queryFn: async (): Promise<ChannelMessage[]> =>
        (await getJson<{ messages: ChannelMessage[] }>(`/api/channels/${cid}/messages`)).messages,
    }
  })
}

/** One thread (root + replies). Refreshes on the channel's SSE ticks because
 *  the key shares the 'channel-messages' prefix the events handler invalidates. */
export function useThreadMessages(channelId: MaybeGetter<string | null>, rootId: MaybeGetter<string | null>) {
  return createQuery(() => {
    const cid = resolve(channelId)
    const rid = resolve(rootId)
    return {
      queryKey: ['channel-messages', cid, 'thread', rid],
      enabled: !!cid && !!rid,
      queryFn: async (): Promise<ChannelMessage[]> =>
        (await getJson<{ messages: ChannelMessage[] }>(`/api/channels/${cid}/messages?thread=${rid}`)).messages,
    }
  })
}

/** Live refresh — one SSE subscription per open channel. `onMessage` rides
 *  the same tick for embedders that own state beyond the transcript (a task
 *  room's comment count, say) without a second subscription. */
export function useChannelEvents(id: MaybeGetter<string | null>, onMessage?: () => void) {
  const qc = useQueryClient()
  $effect(() => {
    const cid = resolve(id)
    if (!cid) return
    const es = new EventSource(`/api/channels/${cid}/events`)
    es.onmessage = (e) => {
      const ev = JSON.parse(e.data as string) as { type: 'message' | 'channel' }
      if (ev.type === 'message') onMessage?.()
      void qc.invalidateQueries({ queryKey: ev.type === 'message' ? ['channel-messages', cid] : ['channel', cid] })
      if (ev.type === 'channel') void qc.invalidateQueries({ queryKey: ['channels'] })
    }
    return () => es.close()
  })
}

export const createChannel = async (name: string, kind: 'channel' | 'group' = 'channel', topic?: string | null): Promise<Channel> =>
  (await postJson<{ channel: Channel }>('/api/channels', { name, kind, topic: topic ?? null })).channel

/** Find-or-create the DM with a teammate. */
export const openDm = async (userId: string): Promise<Channel> =>
  (await postJson<{ channel: Channel }>('/api/dms', { userId })).channel

export const markChannelRead = async (id: string, seq: number): Promise<void> => {
  await postJson<{ ok: true }>(`/api/channels/${id}/read`, { seq })
}

export const sendChannelMessage = async (
  id: string,
  content: string,
  attachmentIds: string[] = [],
  refs: Array<{ type: 'kb-doc' | 'artifact'; id: string }> = [],
  threadRootId: string | null = null,
): Promise<void> => {
  await postJson<{ message: ChannelMessage }>(`/api/channels/${id}/messages`, { content, attachmentIds, refs, threadRootId })
}

export const toggleMessageReaction = async (channelId: string, messageId: string, emoji: string): Promise<void> => {
  await postJson<{ ok: true }>(`/api/channels/${channelId}/messages/${messageId}/reactions`, { emoji })
}

export const editChannelMessage = async (channelId: string, messageId: string, content: string): Promise<void> => {
  await patchJson<{ ok: true }>(`/api/channels/${channelId}/messages/${messageId}`, { content })
}

export const deleteChannelMessage = async (channelId: string, messageId: string): Promise<void> => {
  await delJson<{ ok: true }>(`/api/channels/${channelId}/messages/${messageId}`)
}

export const updateChannel = async (id: string, patch: { name?: string; topic?: string | null }): Promise<void> => {
  await putJson<{ ok: true }>(`/api/channels/${id}`, patch)
}

export const deleteChannel = async (id: string): Promise<void> => {
  await delJson<{ ok: true }>(`/api/channels/${id}?hard=1`)
}

export const addChannelMember = async (id: string, email: string): Promise<void> => {
  await postJson<{ ok: true }>(`/api/channels/${id}/members`, { email })
}
export const removeChannelMember = async (id: string, userId: string): Promise<void> => {
  await delJson<{ ok: true }>(`/api/channels/${id}/members`, { userId })
}
export const addChannelAgent = async (id: string, model: string): Promise<void> => {
  await postJson<{ ok: true }>(`/api/channels/${id}/agents`, { model })
}
export const removeChannelAgent = async (id: string, model: string): Promise<void> => {
  await delJson<{ ok: true }>(`/api/channels/${id}/agents`, { model })
}
