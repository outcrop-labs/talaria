import { createInfiniteQuery, createQuery } from '@tanstack/svelte-query'
import type {
  FocusActionResult,
  FocusQueue,
  FocusSourceType,
  InboxCommandEvent,
  InboxConversationPage,
} from '@/server/inbox-focus'

export type {
  FocusAction,
  FocusActionResult,
  FocusAssistant,
  FocusCommandResponse,
  FocusItem,
  FocusQueue,
  InboxCommandEvent,
  InboxConversationPage,
  InboxTimelineEntry,
} from '@/server/inbox-focus'

const requestSignal = (signal?: AbortSignal): AbortSignal => {
  const timeout = AbortSignal.timeout(20_000)
  return signal ? AbortSignal.any([signal, timeout]) : timeout
}

async function getJson<T>(url: string, signal?: AbortSignal): Promise<T> {
  const response = await fetch(url, { credentials: 'same-origin', signal: requestSignal(signal) })
  if (!response.ok) throw new Error(`Request failed (${response.status})`)
  return response.json() as Promise<T>
}

async function sendJson<T>(url: string, method: 'POST' | 'PUT', body: unknown): Promise<T> {
  const response = await fetch(url, {
    method,
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
    signal: requestSignal(),
  })
  const value = (await response.json().catch(() => null)) as T | { error?: string; message?: string } | null
  if (!response.ok && !(value && typeof value === 'object' && 'status' in value)) {
    const errorValue = value && typeof value === 'object' ? (value as { error?: string; message?: string }) : null
    const detail = errorValue?.message ?? errorValue?.error
    throw new Error(detail || `Request failed (${response.status})`)
  }
  if (!value) throw new Error('The server returned an empty response.')
  return value as T
}

/** A reactive argument: pass plain options, or a getter when `enabled` should
 *  track component state. */
type MaybeGetter<T> = T | (() => T)
const resolve = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export function useInboxFocus(options: MaybeGetter<{ enabled?: boolean }> = {}) {
  return createQuery(() => ({
    queryKey: ['inbox-focus'],
    queryFn: ({ signal }: { signal: AbortSignal }) => getJson<FocusQueue>('/api/inbox/focus', signal),
    enabled: resolve(options).enabled,
    refetchInterval: 30_000,
    staleTime: 10_000,
  }))
}

export function useInboxFocusSummary(options: MaybeGetter<{ enabled?: boolean }> = {}) {
  return createQuery(() => ({
    queryKey: ['inbox-focus-summary'],
    queryFn: ({ signal }: { signal: AbortSignal }) => getJson<{ count: number }>('/api/inbox/focus/summary', signal),
    enabled: resolve(options).enabled,
    refetchInterval: 30_000,
    staleTime: 10_000,
  }))
}

/** One conversation instance's timeline. Keyed by instance id so switching the
 *  panel's chat picker swaps the cached thread rather than merging two. */
export function useInboxFocusConversation(conversationId: string | null, options: MaybeGetter<{ enabled?: boolean }> = {}) {
  return createInfiniteQuery(() => ({
    queryKey: ['inbox-focus-conversation', conversationId],
    queryFn: ({ pageParam, signal }: { pageParam: string; signal: AbortSignal }) => getJson<InboxConversationPage>(
      `/api/inbox/focus/conversation?conversationId=${encodeURIComponent(conversationId ?? '')}${pageParam ? `&cursor=${encodeURIComponent(pageParam)}` : ''}`,
      signal,
    ),
    initialPageParam: '' as string,
    getNextPageParam: (page: InboxConversationPage) => page.nextCursor ?? undefined,
    enabled: resolve(options).enabled,
    staleTime: 5_000,
  }))
}

// ── Conversation instances (the panel's chat picker) ─────────────────────────

export interface InboxConversationSummary {
  id: string
  preview: string
  createdAt: string
  updatedAt: string
}

export function useInboxConversations() {
  return createQuery(() => ({
    queryKey: ['inbox-conversations'],
    queryFn: (): Promise<{ conversations: InboxConversationSummary[] }> =>
      getJson<{ conversations: InboxConversationSummary[] }>('/api/inbox/focus/conversations'),
    staleTime: 10_000,
  }))
}

export async function createInboxConversation(): Promise<string> {
  const out = await sendJson<{ conversation: { id: string } }>('/api/inbox/focus/conversations', 'POST', {})
  return out.conversation.id
}

export async function archiveInboxConversation(id: string): Promise<void> {
  const response = await fetch(`/api/inbox/focus/conversations/${encodeURIComponent(id)}`, {
    method: 'DELETE',
    credentials: 'same-origin',
    signal: requestSignal(),
  })
  if (!response.ok) throw new Error(`Request failed (${response.status})`)
}

export function updateInboxFocusState(input: {
  sourceType: FocusSourceType
  sourceId: string
  snoozedUntil?: string | null
  viewed?: boolean
}) {
  return sendJson<{ ok: true; timelineEntry?: import('@/server/inbox-focus').InboxTimelineEntry }>('/api/inbox/focus/state', 'PUT', input)
}

export function runInboxFocusAction(input: {
  key?: string
  actionId?: string
  payload?: unknown
  commandDecisionId?: string
  decisionId?: string
  confirmationToken?: string
  cancelDecisionId?: string
  undoDecisionId?: string
}) {
  return sendJson<FocusActionResult>('/api/inbox/focus/actions', 'POST', input)
}

export async function* streamInboxFocusCommand(
  input: {
    key?: string | null
    instruction: string
    surface?: string | null
    delegateModel?: string | null
    responseModel?: string | null
    mode?: 'normal' | 'fast' | 'plan'
    /** Reasoning effort for the reply ('' / null = the model's default). */
    effort?: string | null
    attachmentIds?: string[]
    refs?: Array<{ type: 'kb-doc' | 'artifact'; id: string }>
    /** Which conversation instance (the panel's chat picker). */
    conversationId?: string | null
  },
  signal?: AbortSignal,
): AsyncGenerator<InboxCommandEvent> {
  const response = await fetch('/api/inbox/focus/command', {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(input),
    signal,
  })
  if (!response.ok || !response.body) {
    const value = await response.json().catch(() => null) as { error?: string; message?: string } | null
    throw new Error(value?.message ?? value?.error ?? `Request failed (${response.status})`)
  }
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ''
  while (true) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    let separator = buffer.indexOf('\n\n')
    while (separator >= 0) {
      const frame = buffer.slice(0, separator)
      buffer = buffer.slice(separator + 2)
      const data = frame.split('\n').filter((line) => line.startsWith('data:')).map((line) => line.slice(5).trim()).join('\n')
      if (data) yield JSON.parse(data) as InboxCommandEvent
      separator = buffer.indexOf('\n\n')
    }
  }
}
