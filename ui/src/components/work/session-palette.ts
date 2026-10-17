// The jump-to-a-session palette's arithmetic: what a typed query matches, and
// where the arrow keys land. Pure, and separate from SessionPalette.svelte for
// the usual reason — the filter is the part that can be wrong in a way a
// person notices, and it is the part a test can reach.

import type { Conversation } from '@/lib/conversations.svelte'

/** What a query is matched against: the thread's name, and the shelf it sits
 *  on. The group is in there because the rail puts it on screen as a heading —
 *  somebody who filed six threads under "Billing" will reach for that word
 *  before any one title. */
const haystack = (c: Conversation): string =>
  `${c.title ?? ''} ${c.group ?? ''}`.toLowerCase()

/** The palette's list for a query.
 *
 *  EVERY WORD HAS TO LAND, in any order: "bill drive" finds "Drive billing
 *  audit", because a half-remembered name is typed in the order it comes back,
 *  not the order it was written. Each term is a substring — no fuzzy matching,
 *  so a result can always be explained by pointing at the letters.
 *
 *  Pinned first, and otherwise the order it was handed (the rail's, which is
 *  the server's). An empty query lists everything, which is what makes the
 *  palette a session list as well as a search. */
export function filterSessions(convs: Conversation[], query: string): Conversation[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean)
  const matched =
    terms.length === 0
      ? convs
      : convs.filter((c) => {
          const text = haystack(c)
          return terms.every((t) => text.includes(t))
        })
  // Two passes rather than a sort: a stable partition cannot reorder rows that
  // agree on pinned, and the order they arrive in is already meaningful.
  return [...matched.filter((c) => c.pinned), ...matched.filter((c) => !c.pinned)]
}

/** Where an arrow key lands. WRAPS, because the list is short and a palette
 *  that stops at the bottom makes the last row the hardest to reach. Answers 0
 *  for an empty list so the caller never holds an index it cannot render. */
export function nextIndex(current: number, delta: number, length: number): number {
  if (length <= 0) return 0
  return (((current + delta) % length) + length) % length
}
