// The rail badges' client state, lifted out of NavRail.svelte: the Inbox
// count (full queue on the Inbox, summary everywhere else) and the
// /api/unreads counts (Comms, Plan, Research), both under the unread-null
// doctrine — `null` means the count could NOT be read, which is a different
// fact from zero and must not render as one. The rail is where a person
// checks whether anything is waiting without opening anything; a silent 0
// over a failed read is this app's oldest bug shape, on the one surface that
// gates every human decision. `!` with a title is the honest badge.
//
// Runes and queries live here so both nav surfaces — the rail today, the top
// dock next — mount ONE set of queries, not two. The component passes the
// live pathname as a getter so the Inbox/summary swap stays reactive; the
// doctrine lives with the counts and never has to be restated per surface.
import { useInboxFocus, useInboxFocusSummary } from '@/lib/inbox-focus.svelte'
import { shouldAttachInboxDecision } from '@/lib/inbox-focus-surface'
import type { NavItem } from '@/lib/nav'
import { useUnreads } from '@/lib/unreads.svelte'

export interface NavBadges {
  /** The Inbox badge's count, or null when the read failed — the one badge
   *  the unreads query does not carry (its source is a different query with
   *  its own doctrine, see below). */
  readonly inboxUnread: number | null
  /** An item's badge count: undefined = carries none; null = unreadable. */
  badgeFor(item: NavItem): number | null | undefined
}

/** Wire the badge reads for a nav surface. `pathname` is a getter (the live
 *  route) because the Inbox badge swaps between the full queue and the
 *  summary as the person moves — the same predicate the assistant surface
 *  uses, not a fourth spelling of it. Bare `/home` renders the Inbox as its
 *  default tab, so an `isUnder('/home/inbox')` of its own would quietly load
 *  the summary instead of the queue on the URL the nav itself points at. */
export function useNavBadges(pathname: () => string): NavBadges {
  // Are we ON the Inbox? The full queue loads here and only a count
  // elsewhere.
  const isInbox = $derived(shouldAttachInboxDecision(pathname(), undefined))
  const inboxQueue = useInboxFocus(() => ({ enabled: isInbox }))
  const inboxSummary = useInboxFocusSummary(() => ({ enabled: !isInbox }))
  const inboxRead = $derived(isInbox ? inboxQueue : inboxSummary)
  const inboxUnread: number | null = $derived(
    inboxRead.isError && inboxRead.data === undefined
      ? null
      : isInbox
        ? (inboxQueue.data?.counts.total ?? 0)
        : (inboxSummary.data?.count ?? 0),
  )

  // The other badges — Comms, Plan, Research — ride /api/unreads, the same
  // counts their pills show, live over the firehose with its own 30s floor.
  // The SAME unread-null doctrine as the Inbox above applies, for the same
  // reason and on the same surface.
  const unreadsQuery = useUnreads()

  return {
    get inboxUnread() {
      return inboxUnread
    },
    badgeFor(item: NavItem): number | null | undefined {
      if (item.to === '/home') return inboxUnread
      if (!item.badge) return undefined
      if (unreadsQuery.isError && unreadsQuery.data === undefined) return null
      return unreadsQuery.data?.[item.badge] ?? 0
    },
  }
}

/** The title a badge's `!` carries, or undefined for an honest count: a
 *  failed read must say it failed, and nothing else may explain itself. */
export function badgeTitle(badge: number | null): string | undefined {
  return badge === null ? 'Could not load what is waiting here' : undefined
}