// THE WORKSPACE PANE'S TABS — what the pane beside a work session is showing.
//
// A session's workspace is never one surface. There are the files the session
// works on (the Talaria documents, sheets and pages and the Google files the
// agent created or opened, `lib/session-files`), a plan's own document, the run
// report, the drafts it is holding for a person to sign off. Each is a tab
// here, and the set GROWS WHILE THE AGENT WORKS: when a refetch of the
// session's files lists a new one, or one the agent touched again, the harness
// calls `syncTouched`, which opens a tab for it or focuses the one already open.
//
// THE LIST IS NOT STATE WE OWN — it is what the session has, derived from the
// session and held in memory for exactly as long as the session is on screen.
// It is deliberately never persisted: a reload re-derives it from the
// session's own files, so a tab pointing at an artifact somebody deleted
// cannot outlive the refresh. The one thing worth carrying across a reload is
// WHICH tab you were reading, and that is a per-viewer convenience — one
// localStorage key, best effort, with no correctness resting on it. (Which
// files a viewer CLOSED is the other one, and it lives with the files:
// `hideFile` in `lib/session-files`.)

import { readStored, writeStored } from './persist'
import type { FileGlyph } from './session-files'

/** `doc` is a plan session's living document; `artifact` and `google` are the
 *  session's files. */
export type WorkspaceTabKind = 'doc' | 'artifact' | 'google' | 'report' | 'drafts'

/** The small icon a tab wears: the file's type, or what the tab is. */
export type TabGlyph = FileGlyph | 'plan' | 'report' | 'drafts'

export interface WorkspaceTab {
  /** Stable per document, because it is the dedupe key: the same artifact
   *  opened from a chip, from the stream and from the report is one tab. */
  id: string
  kind: WorkspaceTabKind
  label: string
  /** What the body needs in order to render this one — an artifact id, a
   *  Google file id, a doc path. Opaque here; the pane's caller reads it. */
  ref?: string
  /** The strip's icon for it. Absent renders no icon. */
  glyph?: TabGlyph
}

/** One session's strip, as a reader sees it. `active` comes resolved, so no
 *  caller has to re-derive "the active one, or the first, or nothing". */
export interface SessionTabs {
  tabs: WorkspaceTab[]
  activeId: string | null
  active: WorkspaceTab | null
}

interface Slot {
  tabs: WorkspaceTab[]
  /** The tab this store last focused. */
  activeId: string | null
  /** The tab the viewer was reading on their LAST visit, read back from
   *  storage when the slot is created. It outranks `activeId` while it is on
   *  screen, and only until somebody acts — see `resolveActive`. */
  remembered: string | null
}

const bySession = $state<Record<string, Slot>>({})

// ONE KEY FOR EVERY SESSION — a map rather than a key per session, for the
// reason the cap below exists: per-session keys are a slow leak nobody ever
// cleans up, whereas one map can drop its stalest entries as it is written.
const ACTIVE_KEY = 'talaria:workspace-tab:v1'
const REMEMBERED_LIMIT = 40

const EMPTY: SessionTabs = { tabs: [], activeId: null, active: null }

function parseRemembered(raw: unknown): Record<string, string> | null {
  if (raw === null || typeof raw !== 'object' || Array.isArray(raw)) return null
  const out: Record<string, string> = {}
  for (const [sessionId, tabId] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof tabId === 'string' && tabId !== '') out[sessionId] = tabId
  }
  return out
}

const readRemembered = (): Record<string, string> => readStored(ACTIVE_KEY, parseRemembered, {})

/** Record which tab a session is on — or forget the session when `tabId` is
 *  null, because "no preference" is the absent entry and not a sentinel. */
function remember(sessionId: string, tabId: string | null): void {
  const all = readRemembered()
  delete all[sessionId]
  // Insertion order is the only recency this map has, so the session just
  // touched goes last and the stalest entries fall off the front.
  const entries = Object.entries(all)
  if (tabId) entries.push([sessionId, tabId])
  writeStored(ACTIVE_KEY, Object.fromEntries(entries.slice(-REMEMBERED_LIMIT)))
}

/** THE REMEMBERED TAB WINS WHILE IT IS ON SCREEN. The list arrives a tab at a
 *  time as the pane mounts, so without this the active tab would be whichever
 *  one happened to be seeded last — which is to say the memory would never be
 *  observable at all. It stops winning the moment somebody acts: a selection,
 *  a close, the agent touching a document, or a deliberate re-open. */
function resolveActive(slot: Slot): WorkspaceTab | null {
  const byId = (id: string | null) => (id ? slot.tabs.find((t) => t.id === id) : undefined)
  return byId(slot.remembered) ?? byId(slot.activeId) ?? slot.tabs[0] ?? null
}

/** Mutators only: a read must never create a slot, because creating one is a
 *  state mutation and reads happen inside `$derived`. */
function slotFor(sessionId: string): Slot {
  // The slot is read BACK out of the record rather than returned as written:
  // a mutation has to go through `$state`'s proxy or nothing re-renders.
  bySession[sessionId] ??= { tabs: [], activeId: null, remembered: readRemembered()[sessionId] ?? null }
  return bySession[sessionId]
}

/** Persist what the viewer is actually looking at, whichever field put it
 *  there — the resolved tab is the only id worth reading back. */
function store(sessionId: string, slot: Slot): void {
  remember(sessionId, resolveActive(slot)?.id ?? null)
}

/** The session's strip. A session with no tabs and no session at all both
 *  answer the empty strip, so the pane has one shape to render. */
export function tabsFor(sessionId: string | null | undefined): SessionTabs {
  const slot = sessionId ? bySession[sessionId] : undefined
  if (!slot) return EMPTY
  const active = resolveActive(slot)
  return { tabs: slot.tabs, activeId: active?.id ?? null, active }
}

/** Open `tab`, or focus the one already open under its id. */
export function openTab(sessionId: string, tab: WorkspaceTab): void {
  const slot = slotFor(sessionId)
  const at = slot.tabs.findIndex((t) => t.id === tab.id)
  const existing = at < 0 ? undefined : slot.tabs[at]
  if (existing) {
    // MERGED, not replaced: a second open carries the fresher label but may
    // know less than the first did (a touch from the stream has no `ref`),
    // and dropping the ref would blank a body that was rendering fine.
    slot.tabs[at] = { ...existing, ...tab }
    // Pointing at a tab that is already on screen is somebody acting, so the
    // last visit's memory stops outranking it.
    slot.remembered = null
  } else {
    slot.tabs.push(tab)
  }
  slot.activeId = tab.id
  store(sessionId, slot)
}

/** PUT TABS ON THE STRIP WITHOUT TAKING THE PANE. What a session already had
 *  when it was opened arrives as one list, and none of it is news: tabs not
 *  yet open are appended in order, ones already open take the fresher fields,
 *  and nobody's reading moves. `focus` names where a session with no active
 *  tab yet should land — it never overrides one that has an active tab, and
 *  the last visit's memory still outranks it (see `resolveActive`), which is
 *  how "remembered, else `focus`, else the first" falls out of one call. */
export function seedTabs(sessionId: string, tabs: readonly WorkspaceTab[], focus: string | null = null): void {
  const slot = slotFor(sessionId)
  for (const tab of tabs) {
    const at = slot.tabs.findIndex((t) => t.id === tab.id)
    if (at < 0) slot.tabs.push(tab)
    else slot.tabs[at] = { ...slot.tabs[at]!, ...tab }
  }
  if (slot.activeId === null && focus && slot.tabs.some((t) => t.id === focus)) slot.activeId = focus
  store(sessionId, slot)
}

/** Close a tab. Every tab closes (see `isClosable`). */
export function closeTab(sessionId: string, tabId: string): void {
  const slot = bySession[sessionId]
  if (!slot) return
  const at = slot.tabs.findIndex((t) => t.id === tabId)
  const target = at < 0 ? undefined : slot.tabs[at]
  if (!target || !isClosable(target)) return
  const wasActive = resolveActive(slot)?.id === tabId
  slot.tabs.splice(at, 1)
  if (wasActive) {
    // Closing the tab you are reading lands on its NEIGHBOUR — the one that
    // slid into its place, or the one before it when the strip ended there —
    // rather than throwing you back to the start of the strip.
    slot.remembered = null
    slot.activeId = (slot.tabs[at] ?? slot.tabs[at - 1])?.id ?? null
  }
  store(sessionId, slot)
}

/** NAME A TAB THAT IS ALREADY OPEN, and touch nothing else.
 *
 *  The strip's labels arrive AFTER its tabs do: a touch from the stream carries
 *  an id, and the document's own title is a read away. The tab cannot wait for
 *  it — the way back to what the agent just wrote is the point of the tab — so
 *  it opens under a generic name and is renamed here when the title lands.
 *  Deliberately not `openTab`: taking focus for a late-arriving NAME would yank
 *  the pane out from under somebody who moved on while the read was in flight.
 *  An unknown id is ignored (the tab may have been closed), and a label that is
 *  already right is not rewritten. */
export function labelTab(sessionId: string, tabId: string, label: string): void {
  const slot = bySession[sessionId]
  const at = slot?.tabs.findIndex((t) => t.id === tabId) ?? -1
  const tab = slot && at >= 0 ? slot.tabs[at] : undefined
  if (!slot || !tab || tab.label === label) return
  slot.tabs[at] = { ...tab, label }
}

/** Focus a tab that is already open; an unknown id is ignored. */
export function selectTab(sessionId: string, tabId: string): void {
  const slot = bySession[sessionId]
  if (!slot || !slot.tabs.some((t) => t.id === tabId)) return
  slot.remembered = null
  slot.activeId = tabId
  store(sessionId, slot)
}

/** THE AGENT'S CALL: an agent just touched a file, so the pane shows it —
 *  opening a tab for it or focusing the one already open, never a second copy.
 *  It always takes focus, past the last visit's memory too: following the
 *  agent as it works is what the pane is for. */
export function syncTouched(sessionId: string, touched: WorkspaceTab): void {
  openTab(sessionId, touched)
  const slot = bySession[sessionId]
  if (!slot) return
  slot.remembered = null
  store(sessionId, slot)
}

/** EVERY TAB CLOSES. There used to be one that could not — a work session's
 *  living document, the pane's fallback — and a work session has no such
 *  document any more: its pane is its files, and closing one hides it for
 *  this viewer (`hideFile`). A plan's document and a run's report re-open
 *  themselves when the strip empties, which is the harness's rule, not the
 *  store's. Kept as a predicate so the chrome asks one place. */
export const isClosable = (_tab: WorkspaceTab): boolean => true
