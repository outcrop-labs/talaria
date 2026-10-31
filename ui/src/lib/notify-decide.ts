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
//   · this tab is the leader              → an OS banner, after a short claim
//                                           window in which a focused tab the
//                                           registry has not heard from yet
//                                           can still say "mine"
//   · otherwise                           → nothing (the leader has it)
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
  /** This tab, right now. */
  self: { visible: boolean; focused: boolean }
  /** The other tabs' last announcements (never this tab's own). */
  others: readonly TabState[]
  /** This tab holds the leader lock (or decides alone: no locks / channel). */
  isLeader: boolean
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
  if (i.isLeader) return 'banner'
  return 'none'
}
