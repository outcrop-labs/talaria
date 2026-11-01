// ONE ALERT PER NOTIFICATION, ON THE DEVICE THE PERSON IS ON — the decision
// each tab makes for each arrival. Pure, so the whole matrix is a unit test.
//
// Every open Talaria tab learns of a new row at the same moment (the hub
// rebroadcasts the stream to all of them, and each has the poll), and each
// decides — in this order, the first rule that answers wins:
//
//   1. the row is already read             → nothing, anywhere
//   2. its `alert` is `feed` (I-15: the     → nothing: recorded, counted in
//      person's matrix or the producer        the bell, never toasted or
//      said "quiet")                          bannered
//   3. Do Not Disturb is on and the class  → nothing (I-14, I-15); a
//      does not break through                 needs-you row still alerts
//   4. VIEWING: this tab is looking        → nothing, and the row is marked
//      (shown, focused, touched in the        read: the person is reading
//      last 60 s) at the row's own page       the thing it is about (EB-13).
//                                             A hidden tab never qualifies (C8)
//   5. THIS tab is focused and visible     → the in-app toast, here; it tells
//                                           the others it has it (`claimed`)
//   6. ANOTHER fresh tab of this browser   → nothing here: that tab toasts
//      is focused+visible
//   7. CROSS-DEVICE: the presence winner   → nothing: the person is on that
//      (I-17: the client the person is        device, and it alerts there
//      looking at, else active on) is a
//      client of ANOTHER device
//   then wave 1's tail, unchanged —
//   · banners are OFF in this browser     → the in-app toast in ONE visible
//     (not granted — everyone the soft      tab: the one touched last (ties
//     ask is for — a plain-http origin,     to the lowest tab id), so a
//     or switched off here)                 Talaria window on a second
//                                           monitor still shows it while the
//                                           person types elsewhere; nothing
//                                           when no tab is visible
//   · this tab is the leader              → an OS banner, after a short claim
//                                           window in which a focused tab the
//                                           registry has not heard from yet
//                                           can still say "mine"
//   · otherwise                           → nothing (the leader has it)
//
// Rule 7 compares the winner with THIS browser's clients (this tab's own and
// every fresh tab's announced one) and with the other clients that ARE this
// device (`sameDevice`: inside the desktop shell, the shell's own watcher for
// this instance, `desk-<local instance id>` — it beats while the page is
// suspended, and its last `active` outlives a restore by up to 75 s): a
// winner among them is this device, and the tail decides which tab. With no
// presence (an api without it, or no winner — the person is idle
// everywhere), rule 7 does not apply and the tail is wave 1's per-browser
// rule.
//
// Why "touched last" and not "every visible tab": each tab knows its own
// last input exactly and the others' only as of their last announcement
// (never newer), so the true latest tab always sees itself winning — the
// arrival can be shown twice in a race, never zero times.
//
// The service worker's push is the other source of banners for the same row;
// the shared tag namespace and the bannered store (notify-bannered.ts) make
// the second one a no-op.

import { LOOKING_INPUT_MS } from './looking'

/** One tab, as it last announced itself on the channel. */
export interface TabState {
  tabId: string
  visible: boolean
  focused: boolean
  /** Last pointer or key input in that tab, ms epoch (0 = never). */
  lastInputAt: number
  /** When it announced this, ms epoch. */
  at: number
  /** Its presence client id (notify-presence.ts); absent from an older build. */
  clientId?: string
  /** It holds the leader lock (as of this announcement). */
  leader?: boolean
  /** It runs a hub that rebroadcasts the stream (notify-hub.ts). */
  hub?: boolean
}

/** A registry entry older than this is a tab that went away without a
 *  goodbye (a crash, a frozen background page): it does not count. */
export const STALE_MS = 15_000

/** How long the leader waits for a focused tab to claim an arrival before it
 *  posts the banner itself. */
export const CLAIM_WINDOW_MS = 300

/** `seen`: nothing is shown, and the row is marked read (rule 4). */
export type Decision = 'none' | 'toast' | 'banner' | 'seen'

export interface DecideInput {
  /** The row is already read (somewhere). */
  read: boolean
  /** The row's `alert` (I-15); null or absent is `banner`. */
  alert?: 'banner' | 'feed' | null
  /** Do Not Disturb, as the server evaluated it (I-14); absent is off. */
  dnd?: { active: boolean } | null
  /** The row's class breaks through DND (a needs-you class). */
  breaksThrough?: boolean
  /** The row's href path equals this tab's path (rule 4). */
  atRowPath?: boolean
  /** This tab, right now. `tabId` and `lastInputAt` pick the one visible tab
   *  that toasts when banners are off; `lastInputAt` also decides LOOKING
   *  (rule 4); `clientId` is this tab's presence client (rule 7). */
  self: { visible: boolean; focused: boolean; tabId?: string; lastInputAt?: number; clientId?: string }
  /** The other tabs' last announcements (never this tab's own). */
  others: readonly TabState[]
  /** This tab holds the leader lock (or decides alone: no locks / channel). */
  isLeader: boolean
  /** This browser can post an OS banner (`browserNotifyEnabled()`): granted,
   *  and not switched off here. Omitted means yes. */
  bannersOn?: boolean
  /** The client the person is on, per the server (I-17: looking, else
   *  active), or null/absent: no presence — rule 7 does not apply. */
  winner?: string | null
  /** Presence clients that are THIS device besides this browser's tabs: in
   *  the desktop shell, its watcher for this instance (notify-presence.ts
   *  `deskClientId`). */
  sameDevice?: readonly string[]
  now: number
}

/** Is that tab one the person is looking at, as of `now`? */
export function isLookedAt(t: TabState, now: number): boolean {
  return t.visible && t.focused && now - t.at <= STALE_MS
}

/** Does Do Not Disturb hold arrivals NOW? The server computes `active` when
 *  the list is read, and the list can be a poll old (a minute, in a page
 *  hidden long enough for the browser to throttle it). A pause or a quiet
 *  period that has ENDED since (its `until` is past) holds nothing: else the
 *  first arrival after it ends would be recorded silently and never alerted.
 *  (One that has started since the read is the server's to hold: it files
 *  rows written during DND as `feed`.) */
export function dndHolds(dnd: { active?: boolean; until?: string | null } | null | undefined, now: number): boolean {
  if (!dnd?.active) return false
  if (!dnd.until) return true
  const end = Date.parse(dnd.until)
  return !Number.isFinite(end) || end > now
}

/** Is this tab LOOKING: shown, focused, and touched within 60 s (I-17)? */
export function selfLooking(self: DecideInput['self'], now: number): boolean {
  const at = self.lastInputAt ?? 0
  return self.visible && self.focused && at > 0 && now - at <= LOOKING_INPUT_MS
}

/** Is the presence winner one of THIS device's clients: this tab, a fresh
 *  tab of this browser, or a client named in `sameDevice`? */
export function winnerIsHere(i: Pick<DecideInput, 'winner' | 'self' | 'others' | 'now' | 'sameDevice'>): boolean {
  if (!i.winner) return true
  if (i.self.clientId && i.winner === i.self.clientId) return true
  if (i.sameDevice?.includes(i.winner)) return true
  return i.others.some((t) => t.clientId === i.winner && i.now - t.at <= STALE_MS)
}

/** The decision for one arrival in one tab. A `banner` is posted only after
 *  the claim window, and only if no tab claimed the row meanwhile. */
export function decide(i: DecideInput): Decision {
  if (i.read) return 'none'
  if (i.alert === 'feed') return 'none'
  if (i.dnd?.active && !i.breaksThrough) return 'none'
  if (i.atRowPath && selfLooking(i.self, i.now)) return 'seen'
  if (i.self.visible && i.self.focused) return 'toast'
  if (i.others.some((t) => isLookedAt(t, i.now))) return 'none'
  if (!winnerIsHere(i)) return 'none'
  if (i.bannersOn === false) return i.self.visible && touchedLastOfVisible(i) ? 'toast' : 'none'
  if (i.isLeader) return 'banner'
  return 'none'
}

/** The path part of an href on this origin, or null (no link, another
 *  origin, garbage). */
export function hrefPath(href: string | null | undefined, origin: string): string | null {
  const h = (href ?? '').trim()
  if (!h.startsWith('/') || h.startsWith('//')) return null
  try {
    const u = new URL(h, origin)
    if (u.origin !== new URL(origin).origin) return null
    return u.pathname.replace(/\/+$/, '') || '/'
  } catch {
    return null
  }
}

/** Does this row point at the page this tab is on? */
export function atPath(href: string | null | undefined, pathname: string, origin: string): boolean {
  const p = hrefPath(href, origin)
  if (!p) return false
  return p === (pathname.replace(/\/+$/, '') || '/')
}

/** The rows a visit reads (EB-13): unread DIRECT and FOLLOWING rows pointing
 *  at the page the person is now looking at. A needs-you row is cleared by
 *  its resolution (an approval decided), never by a glance; a row whose
 *  category the api does not say (an api before the contract) is left alone. */
export function visitReadIds(
  rows: ReadonlyArray<{ id: string; href?: string | null; readAt: string | null; category?: string | null }>,
  pathname: string,
  origin: string,
): string[] {
  const out: string[] = []
  for (const r of rows) {
    if (r.readAt) continue
    if (r.category !== 'direct' && r.category !== 'following') continue
    if (atPath(r.href, pathname, origin)) out.push(r.id)
  }
  return out
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
