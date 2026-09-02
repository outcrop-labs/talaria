import { createInfiniteQuery, createQuery } from '@tanstack/svelte-query'
import { delJson, getJson, postJson, postJsonOr, postStream, putJson } from '@/lib/fetch-json'
import { sseFrames } from '@/lib/sse-parse'
import type {
  FocusActionResult,
  FocusQueue,
  FocusSourceType,
  InboxCommandEvent,
  InboxConversationPage,
} from './inbox-focus-types'

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
} from './inbox-focus-types'

// The focus endpoints are polled on 30s timers, so every read/mutation here
// carries a 20s abort — a hung request must never stack behind the next tick.
const requestSignal = (signal?: AbortSignal): AbortSignal => {
  const timeout = AbortSignal.timeout(20_000)
  return signal ? AbortSignal.any([signal, timeout]) : timeout
}

/** A reactive argument: pass plain options, or a getter when `enabled` should
 *  track component state. */
type MaybeGetter<T> = T | (() => T)
const resolve = <T,>(v: MaybeGetter<T>): T => (typeof v === 'function' ? (v as () => T)() : v)

export function useInboxFocus(options: MaybeGetter<{ enabled?: boolean }> = {}) {
  return createQuery(() => ({
    queryKey: ['inbox-focus'],
    queryFn: ({ signal }: { signal: AbortSignal }) => getJson<FocusQueue>('/api/inbox/focus', { signal: requestSignal(signal) }),
    enabled: resolve(options).enabled,
    refetchInterval: 30_000,
    staleTime: 10_000,
  }))
}

export function useInboxFocusSummary(options: MaybeGetter<{ enabled?: boolean }> = {}) {
  return createQuery(() => ({
    queryKey: ['inbox-focus-summary'],
    queryFn: ({ signal }: { signal: AbortSignal }) => getJson<{ count: number }>('/api/inbox/focus/summary', { signal: requestSignal(signal) }),
    enabled: resolve(options).enabled,
    refetchInterval: 30_000,
    staleTime: 10_000,
  }))
}

/** One conversation instance's timeline. Keyed by instance id so switching the
 *  panel's chat picker swaps the cached thread rather than merging two. A null
 *  id asks for `current` — the server resolves the caller's own latest
 *  instance, which is what the panel's first load wants. */
export function useInboxFocusConversation(conversationId: string | null, options: MaybeGetter<{ enabled?: boolean }> = {}) {
  return createInfiniteQuery(() => ({
    queryKey: ['inbox-focus-conversation', conversationId],
    queryFn: ({ pageParam, signal }: { pageParam: string; signal: AbortSignal }) => getJson<InboxConversationPage>(
      `${conversationId ? `/api/inbox/focus/conversations/${encodeURIComponent(conversationId)}` : '/api/inbox/focus/conversations/current'}${pageParam ? `?cursor=${encodeURIComponent(pageParam)}` : ''}`,
      { signal: requestSignal(signal) },
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
  const out = await postJson<{ conversation: { id: string } }>('/api/inbox/focus/conversations', {})
  return out.conversation.id
}

export function archiveInboxConversation(id: string): Promise<void> {
  return delJson(`/api/inbox/focus/conversations/${encodeURIComponent(id)}`, { signal: requestSignal() }).then(() => undefined)
}

export function updateInboxFocusState(input: {
  sourceType: FocusSourceType
  sourceId: string
  snoozedUntil?: string | null
  viewed?: boolean
}) {
  return putJson<{ ok: true; timelineEntry?: import('./inbox-focus-types').InboxTimelineEntry }>('/api/inbox/focus/state', input, {
    signal: requestSignal(),
  })
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
  // `stale` (409) and `failed` (422) come back WITH their result body — the
  // result is the answer the panel renders, not a transport error.
  return postJsonOr<FocusActionResult>('/api/inbox/focus/actions', input, [409, 422], { signal: requestSignal() })
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
  const response = await postStream('/api/inbox/focus/command', input, { signal })
  // Same frames, same reader discipline, as the chat stream — sse-parse owns
  // the loop, including the release on an early consumer exit.
  for await (const { data } of sseFrames(response.body)) {
    if (data) yield JSON.parse(data) as InboxCommandEvent
  }
}
