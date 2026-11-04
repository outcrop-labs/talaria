// Is the person looking at this page: shown AND focused? One answer for
// everything that acts on it. The notification runtime toasts only for a page
// the person looks at and otherwise alerts (NotificationToasts.svelte,
// notify-decide.ts); an open conversation counts as read only while it is
// looked at (ChannelView.svelte), because a read also clears its bell rows.
//
// Focus, not visibility alone: a minimized window in WebKitGTK (the desktop
// shell) still reports `visible`, and a window on another monitor is visible
// while the person types somewhere else.

interface LookDoc {
  visibilityState: DocumentVisibilityState
  hasFocus(): boolean
}

export function lookingNow(doc: LookDoc | undefined = typeof document === 'undefined' ? undefined : document): boolean {
  if (!doc) return false
  try {
    return doc.visibilityState === 'visible' && doc.hasFocus()
  } catch {
    return false
  }
}

/** Follow `lookingNow` live: `fn` gets every change (focus, blur, the page
 *  hidden or shown, a back/forward-cache restore). Returns the unsubscribe. */
export function watchLooking(
  fn: (looking: boolean) => void,
  win: EventTarget | undefined = typeof window === 'undefined' ? undefined : window,
  doc: (LookDoc & EventTarget) | undefined = typeof document === 'undefined' ? undefined : document,
): () => void {
  if (!win || !doc) return () => {}
  let last = lookingNow(doc)
  const check = () => {
    const now = lookingNow(doc)
    if (now === last) return
    last = now
    fn(now)
  }
  win.addEventListener('focus', check)
  win.addEventListener('blur', check)
  win.addEventListener('pageshow', check)
  doc.addEventListener('visibilitychange', check)
  return () => {
    win.removeEventListener('focus', check)
    win.removeEventListener('blur', check)
    win.removeEventListener('pageshow', check)
    doc.removeEventListener('visibilitychange', check)
  }
}

/** The read cursor an open conversation posts now, or null for none: the
 *  newest message's seq, once the person is looking and it is past what was
 *  already posted for this channel. Not looking, nothing is posted — the
 *  messages stay unread (and their DM or mention still alerts) until they
 *  look, and then the newest is posted at once. `channelId` is the cursor's
 *  key: the channel for the main feed, the thread's root for a thread panel
 *  (ThreadPanel), whose seqs come from the same channel-wide counter. */
export function readCursorToPost(input: {
  channelId: string
  latest: number
  posted: { id: string; seq: number }
  looking: boolean
}): number | null {
  if (!input.looking || !input.latest) return null
  if (input.posted.id === input.channelId && input.latest <= input.posted.seq) return null
  return input.latest
}

// ── Input: the half of "looking" a document cannot answer ────────────────────
// Shown and focused says a window is in front; it does not say anyone is
// there. A person who walked away from a focused Talaria tab is not looking
// at it, and the device they are on now (their phone's browser, the laptop
// they carried to the meeting) is the one that should alert (I-17). So the
// runtime also asks WHEN the person last touched this page: a pointer, a key,
// the wheel, a touch. One listener set per page, shared by the tab registry
// (notify-leader.ts), the presence heartbeat (notify-presence.ts) and the
// decider (notify-decide.ts).

/** Input within this long, on a shown and focused page, is LOOKING (I-17, O-13). */
export const LOOKING_INPUT_MS = 60_000
/** Input within this long, on a shown page, is ACTIVE (I-17, O-13). */
export const ACTIVE_INPUT_MS = 120_000

let inputAt = 0
let inputRefs = 0
let removeInput: (() => void) | null = null
const inputListeners = new Set<(at: number) => void>()

/** When the person last touched this page, ms epoch (0 = never). */
export function lastInputAt(): number {
  return inputAt
}

/** Record input now (the listeners below call it; so do tests). */
export function noteInput(at: number = Date.now()): void {
  if (at <= inputAt) return
  inputAt = at
  for (const fn of inputListeners) fn(at)
}

/** Hear every input (at most one call per second from pointer moves). */
export function onInput(fn: (at: number) => void): () => void {
  inputListeners.add(fn)
  return () => inputListeners.delete(fn)
}

/** Start listening for input on this page; returns the release. Reference
 *  counted, so every consumer can ask and the page holds one listener set. A
 *  page that opens shown and focused counts its load as input: the person just
 *  opened it. Pointer moves are sampled at most once a second. */
export function trackInput(
  win: (EventTarget & { document?: LookDoc }) | undefined = typeof window === 'undefined' ? undefined : window,
  now: () => number = Date.now,
): () => void {
  if (!win) return () => {}
  inputRefs++
  if (!removeInput) {
    const doc = win.document ?? (typeof document === 'undefined' ? undefined : document)
    if (inputAt === 0 && lookingNow(doc)) noteInput(now())
    let movedAt = 0
    const hit = () => noteInput(now())
    const move = () => {
      const t = now()
      if (t - movedAt < 1000) return
      movedAt = t
      noteInput(t)
    }
    const opts = { capture: true, passive: true } as AddEventListenerOptions
    const kinds = ['pointerdown', 'keydown', 'wheel', 'touchstart'] as const
    for (const k of kinds) win.addEventListener(k, hit, opts)
    win.addEventListener('pointermove', move, opts)
    removeInput = () => {
      for (const k of kinds) win.removeEventListener(k, hit, opts)
      win.removeEventListener('pointermove', move, opts)
    }
  }
  let released = false
  return () => {
    if (released) return
    released = true
    inputRefs--
    if (inputRefs <= 0) {
      inputRefs = 0
      removeInput?.()
      removeInput = null
    }
  }
}

/** Is the person looking at this page: shown, focused, AND touched within
 *  `LOOKING_INPUT_MS`? Stricter than `lookingNow`, which a read cursor uses
 *  (a person reading without touching anything is still reading): this one
 *  decides whether a row may be marked read on sight, and whether this page
 *  is the device the person is on. */
export function lookingWithInput(
  doc: LookDoc | undefined = typeof document === 'undefined' ? undefined : document,
  now: number = Date.now(),
  at: number = inputAt,
): boolean {
  return lookingNow(doc) && at > 0 && now - at <= LOOKING_INPUT_MS
}

/** Test hook: forget the recorded input. */
export function resetInputForTests(): void {
  inputAt = 0
  inputRefs = 0
  removeInput?.()
  removeInput = null
  inputListeners.clear()
}
