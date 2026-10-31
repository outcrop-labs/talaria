// ONE OS BANNER PER NOTIFICATION PER BROWSER — the decision each tab makes
// for each arrival. Pure, so the whole matrix is a unit test.
//
// Every open Talaria tab learns of a new row at the same moment (each has the
// firehose and the poll), and each used to decide alone: every tab toasted,
// and every BACKGROUND tab posted an OS banner — while the person was reading
// Talaria in another tab. The tabs now share a registry (notify-leader.ts:
// who is visible, who is focused) and one of them holds the leader lock:
//
//   · the row is already read             → nothing, anywhere
//   · THIS tab is focused and visible     → the in-app toast, here; it tells
//                                           the others it has it (`claimed`)
//   · ANOTHER fresh tab is focused+visible → nothing here: that tab toasts
//   · banners are OFF in this browser     → the in-app toast in ONE visible
//     (not granted — everyone the soft      tab: the one touched last (ties
//     ask is for — a plain-http origin,     to the lowest tab id), so a
//     or switched off here)                 Talaria window on a second
//                                           monitor still shows it while the
//                                           person types elsewhere; nothing
//                                           when no tab is visible (the bell
//                                           keeps it, and catch-up does not
//                                           re-surface it)
//   · this tab is the leader              → an OS banner, after a short claim
//                                           window in which a focused tab the
//                                           registry has not heard from yet
//                                           can still say "mine"
//   · otherwise                           → nothing (the leader has it)
//
// Why "touched last" and not "every visible tab": each tab knows its own
// last input exactly and the others' only as of their last announcement
// (never newer), so the true latest tab always sees itself winning — the
// arrival can be shown twice in a race, never zero times.
//
// The service worker's push is the other source of banners for the same row;
// the shared tag namespace and the bannered store (notify-bannered.ts) make
// the second one a no-op.

/** One tab, as it last announced itself on the channel. */
export interface TabState {
  tabId: string
  visible: boolean
  focused: boolean
  /** Last pointer or key input in that tab, ms epoch (0 = never). */
  lastInputAt: number
  /** When it announced this, ms epoch. */
  at: number
}

/** A registry entry older than this is a tab that went away without a
 *  goodbye (a crash, a frozen background page): it does not count. */
export const STALE_MS = 15_000

/** How long the leader waits for a focused tab to claim an arrival before it
 *  posts the banner itself. */
export const CLAIM_WINDOW_MS = 300

export type Decision = 'none' | 'toast' | 'banner'

export interface DecideInput {
  /** The row is already read (somewhere). */
  read: boolean
  /** This tab, right now. `tabId` and `lastInputAt` pick the one visible tab
   *  that toasts when banners are off. */
  self: { visible: boolean; focused: boolean; tabId?: string; lastInputAt?: number }
  /** The other tabs' last announcements (never this tab's own). */
  others: readonly TabState[]
  /** This tab holds the leader lock (or decides alone: no locks / channel). */
  isLeader: boolean
  /** This browser can post an OS banner (`browserNotifyEnabled()`): granted,
   *  and not switched off here. Omitted means yes. */
  bannersOn?: boolean
  now: number
}

/** Is that tab one the person is looking at, as of `now`? */
export function isLookedAt(t: TabState, now: number): boolean {
  return t.visible && t.focused && now - t.at <= STALE_MS
}

/** The decision for one arrival in one tab. A `banner` is posted only after
 *  the claim window, and only if no tab claimed the row meanwhile. */
export function decide(i: DecideInput): Decision {
  if (i.read) return 'none'
  if (i.self.visible && i.self.focused) return 'toast'
  if (i.others.some((t) => isLookedAt(t, i.now))) return 'none'
  if (i.bannersOn === false) return i.self.visible && touchedLastOfVisible(i) ? 'toast' : 'none'
  if (i.isLeader) return 'banner'
  return 'none'
}

/** Is this (visible) tab the visible one touched last? Ties go to the lowest
 *  tab id, so two never-touched windows still agree on one. */
function touchedLastOfVisible(i: DecideInput): boolean {
  const mine = { at: i.self.lastInputAt ?? 0, id: i.self.tabId ?? '' }
  return !i.others.some((t) => {
    if (!t.visible || i.now - t.at > STALE_MS) return false
    return t.lastInputAt > mine.at || (t.lastInputAt === mine.at && t.tabId < mine.id)
  })
}
