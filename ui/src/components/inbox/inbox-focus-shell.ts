// Typed context key + accessor for the Inbox focus workspace. The provider
// lives in InboxFocusShell.svelte; anything under it (FocusInbox, AppLayout's
// inbox surfaces) reads the workspace through `useInboxFocusWorkspace()`.
import { getContext } from 'svelte'
import type { FocusAction, FocusItem, FocusQueue } from '@/lib/inbox-focus.svelte'

export const INBOX_SNOOZE_OPTIONS = [
  { label: '1 hour', value: 60 * 60_000 },
  { label: 'Tomorrow', value: 24 * 60 * 60_000 },
  { label: 'Next week', value: 7 * 24 * 60 * 60_000 },
] as const

export interface InboxFocusWorkspaceValue {
  data: FocusQueue | undefined
  isLoading: boolean
  isError: boolean
  /** The rejection itself, not just the fact of one. A queue that failed to
   *  load has to be able to say WHY on the surface — "could not load" with the
   *  server's reason withheld is the same dead end as no message at all. */
  error: unknown
  refetch: () => Promise<unknown>
  orderedItems: FocusItem[]
  active: FocusItem | null
  recommendedAction: FocusAction | null
  busyAction: string | null
  snoozeMs: number
  setSnoozeMs: (value: number) => void
  performAction: (item: FocusItem, actionId: string, payload?: unknown) => Promise<void>
  snooze: () => Promise<void>
  skip: () => void
}

export const INBOX_FOCUS_WORKSPACE_KEY = Symbol('inbox-focus-workspace')

export function useInboxFocusWorkspace(): InboxFocusWorkspaceValue {
  const value = getContext<InboxFocusWorkspaceValue | undefined>(INBOX_FOCUS_WORKSPACE_KEY)
  if (!value) throw new Error('useInboxFocusWorkspace must be used inside InboxFocusShell')
  return value
}
