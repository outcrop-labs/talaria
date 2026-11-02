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
 *  look, and then the newest is posted at once. */
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
