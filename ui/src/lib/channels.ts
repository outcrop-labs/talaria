// Group-chat client: queries + mutations + live SSE refresh.
import { useEffect } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'

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

const j = async <T>(r: Response): Promise<T> => {
  const data = (await r.json().catch(() => null)) as (T & { error?: string }) | null
  if (!r.ok || !data) throw new Error(data?.error ?? `request failed (${r.status})`)
  return data
}

export function useChannels() {
  return useQuery({
    queryKey: ['channels'],
    queryFn: async (): Promise<Channel[]> =>
      (await j<{ channels: Channel[] }>(await fetch('/api/channels', { credentials: 'same-origin' }))).channels,
  })
}

export function useChannelDetail(id: string | null) {
  return useQuery({
    queryKey: ['channel', id],
    enabled: !!id,
    queryFn: async (): Promise<ChannelDetail> =>
      j<ChannelDetail>(await fetch(`/api/channels/${id}`, { credentials: 'same-origin' })),
  })
}

export function useChannelMessages(id: string | null) {
  return useQuery({
    queryKey: ['channel-messages', id],
    enabled: !!id,
    queryFn: async (): Promise<ChannelMessage[]> =>
      (await j<{ messages: ChannelMessage[] }>(await fetch(`/api/channels/${id}/messages`, { credentials: 'same-origin' })))
        .messages,
  })
}

/** One thread (root + replies). Refreshes on the channel's SSE ticks because
 *  the key shares the 'channel-messages' prefix the events handler invalidates. */
export function useThreadMessages(channelId: string | null, rootId: string | null) {
  return useQuery({
    queryKey: ['channel-messages', channelId, 'thread', rootId],
    enabled: !!channelId && !!rootId,
    queryFn: async (): Promise<ChannelMessage[]> =>
      (
        await j<{ messages: ChannelMessage[] }>(
          await fetch(`/api/channels/${channelId}/messages?thread=${rootId}`, { credentials: 'same-origin' }),
        )
      ).messages,
  })
}

/** Live refresh — one SSE subscription per open channel. */
export function useChannelEvents(id: string | null) {
  const qc = useQueryClient()
  useEffect(() => {
    if (!id) return
    const es = new EventSource(`/api/channels/${id}/events`)
    es.onmessage = (e) => {
      const ev = JSON.parse(e.data as string) as { type: 'message' | 'channel' }
      void qc.invalidateQueries({ queryKey: ev.type === 'message' ? ['channel-messages', id] : ['channel', id] })
      if (ev.type === 'channel') void qc.invalidateQueries({ queryKey: ['channels'] })
    }
    return () => es.close()
  }, [id, qc])
}

const post = (url: string, body: unknown, method = 'POST') =>
  fetch(url, {
    method,
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })

export const createChannel = async (name: string, kind: 'channel' | 'group' = 'channel', topic?: string | null): Promise<Channel> =>
  (await j<{ channel: Channel }>(await post('/api/channels', { name, kind, topic: topic ?? null }))).channel

/** Find-or-create the DM with a teammate. */
export const openDm = async (userId: string): Promise<Channel> =>
  (await j<{ channel: Channel }>(await post('/api/dms', { userId }))).channel

export const markChannelRead = async (id: string, seq: number): Promise<void> => {
  await post(`/api/channels/${id}/read`, { seq })
}

export const sendChannelMessage = async (
  id: string,
  content: string,
  attachmentIds: string[] = [],
  refs: Array<{ type: 'kb-doc' | 'artifact'; id: string }> = [],
  threadRootId: string | null = null,
): Promise<void> => {
  await j(await post(`/api/channels/${id}/messages`, { content, attachmentIds, refs, threadRootId }))
}

export const toggleMessageReaction = async (channelId: string, messageId: string, emoji: string): Promise<void> => {
  await j(await post(`/api/channels/${channelId}/messages/${messageId}/reactions`, { emoji }))
}

export const editChannelMessage = async (channelId: string, messageId: string, content: string): Promise<void> => {
  await j(await post(`/api/channels/${channelId}/messages/${messageId}`, { content }, 'PATCH'))
}

export const deleteChannelMessage = async (channelId: string, messageId: string): Promise<void> => {
  await j(await fetch(`/api/channels/${channelId}/messages/${messageId}`, { method: 'DELETE', credentials: 'same-origin' }))
}

export const updateChannel = async (id: string, patch: { name?: string; topic?: string | null }): Promise<void> => {
  await j(await post(`/api/channels/${id}`, patch, 'PUT'))
}

export const deleteChannel = async (id: string): Promise<void> => {
  await j(await fetch(`/api/channels/${id}?hard=1`, { method: 'DELETE', credentials: 'same-origin' }))
}

export const addChannelMember = async (id: string, email: string): Promise<void> => {
  await j(await post(`/api/channels/${id}/members`, { email }))
}
export const removeChannelMember = async (id: string, userId: string): Promise<void> => {
  await j(await post(`/api/channels/${id}/members`, { userId }, 'DELETE'))
}
export const addChannelAgent = async (id: string, model: string): Promise<void> => {
  await j(await post(`/api/channels/${id}/agents`, { model }))
}
export const removeChannelAgent = async (id: string, model: string): Promise<void> => {
  await j(await post(`/api/channels/${id}/agents`, { model }, 'DELETE'))
}
