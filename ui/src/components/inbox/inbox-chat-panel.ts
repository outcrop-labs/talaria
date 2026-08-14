// Shared types + panel-chrome persistence for InboxChatPanel.svelte. The
// localStorage read/write helpers live here (plain TS, no runes) — the
// component mirrors them into $state and subscribes to the sync events.
import type { AssistantMode } from '@/components/inbox/assistant-composer-controls'
import { DEFAULT_INBOX_PANEL_WIDTH, clampInboxPanelWidth } from '@/lib/inbox-panel-size'

export interface InboxChatPanelHandle {
  focus: () => void
  expand: () => void
  insertText: (text: string) => void
}

export interface StreamingTurn {
  user: string
  status: string
  content: string
}

export interface InboxCommandOptions {
  focusKey: string | null
  delegateModel: string | null
  responseModel: string | null
  mode: AssistantMode
  attachmentIds: string[]
  refs: Array<{ type: 'kb-doc' | 'artifact'; id: string }>
}

export const PANEL_COLLAPSED_KEY = 'talaria:inbox-chat-collapsed'
export const PANEL_COLLAPSED_EVENT = 'talaria:inbox-chat-collapsed'
// v2 adopts the 700px composer width from the design spec as the default,
// while retaining resizing.
export const PANEL_WIDTH_KEY = 'talaria:inbox-chat-width-v2'
let collapsedFallback = true
let widthFallback = DEFAULT_INBOX_PANEL_WIDTH

// NO STORED PREFERENCE MEANS CLOSED. This read used to be `=== '1'`, which
// answers "not collapsed" for the absent key — so a first load, on any view,
// opened the assistant over the page the person actually navigated to. Nobody
// asked for it; it just happened to be what "no key yet" decoded to. Opening
// is a decision, and the panel remembers it (`writePanelCollapsed` stores '0'),
// so anyone who has ever opened it still lands open.
export function readPanelCollapsed(): boolean {
  try {
    const stored = window.localStorage.getItem(PANEL_COLLAPSED_KEY)
    return stored === null ? true : stored === '1'
  } catch {
    return collapsedFallback
  }
}

export function subscribePanelCollapsed(onChange: () => void): () => void {
  window.addEventListener(PANEL_COLLAPSED_EVENT, onChange)
  window.addEventListener('storage', onChange)
  return () => {
    window.removeEventListener(PANEL_COLLAPSED_EVENT, onChange)
    window.removeEventListener('storage', onChange)
  }
}

export function writePanelCollapsed(next: boolean): void {
  collapsedFallback = next
  try {
    window.localStorage.setItem(PANEL_COLLAPSED_KEY, next ? '1' : '0')
  } catch {
    /* private mode: keep the in-memory preference for this tab */
  }
  window.dispatchEvent(new Event(PANEL_COLLAPSED_EVENT))
}

// ── Output the user has not seen yet ────────────────────────────────────────
// The panel used to own this as local state and render it as a dot on its own
// collapsed rail. That rail is gone — the assistant is launched from the nav
// sidebar now — so the signal has to outlive the closed panel or the assistant
// can finish a piece of work with nothing anywhere saying so.
//
// Deliberately NOT persisted: "there is something new since you last looked" is
// true of a session, not of a browser profile. A reload has shown you the
// timeline, so restoring the dot would be a standing lie. In-memory plus an
// event is the whole store; the panel writes it, the sidebar reads it.
export const PANEL_UNSEEN_EVENT = 'talaria:inbox-chat-unseen'
let unseenOutput = false

export function readPanelUnseen(): boolean {
  return unseenOutput
}

export function subscribePanelUnseen(onChange: () => void): () => void {
  window.addEventListener(PANEL_UNSEEN_EVENT, onChange)
  return () => window.removeEventListener(PANEL_UNSEEN_EVENT, onChange)
}

export function writePanelUnseen(next: boolean): void {
  if (unseenOutput === next) return
  unseenOutput = next
  window.dispatchEvent(new Event(PANEL_UNSEEN_EVENT))
}

export function readPanelWidth(): number {
  try {
    const stored = window.localStorage.getItem(PANEL_WIDTH_KEY)
    return stored === null ? widthFallback : clampInboxPanelWidth(Number(stored))
  } catch {
    return widthFallback
  }
}

export function writePanelWidth(next: number): void {
  widthFallback = next
  try {
    window.localStorage.setItem(PANEL_WIDTH_KEY, String(next))
  } catch {
    /* private mode: keep the in-memory preference for this tab */
  }
}
