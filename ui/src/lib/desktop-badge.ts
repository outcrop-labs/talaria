// The dock and launcher badge reporter (interface I-5): one instance page's
// unread count, handed to the desktop shell, which keeps a share per instance
// and shows the sum. Debounced, so a burst of refetches is one report — of the
// count the burst SETTLED on: 2 → 3 → 2 inside the window reports nothing new,
// not 3. A signed-out page reports 0, because the query cache still holds the
// last signed-in count and the login screen mounts the switcher too.
//
// The count is For you (EB-11, O-10): Needs you + Direct, the rows that are
// the person's to answer. Following lands in the bell's All tab without
// badging the dock. An instance that predates For you serves only `unread`,
// and that is the count then.

export const BADGE_DEBOUNCE_MS = 500

/** The badge's count from a notifications payload: For you
 *  (`unreadForYou`), else every unread row from an instance that predates
 *  it, else nothing yet (undefined: no read has landed). */
export function badgeCount(
  payload: { unread?: number; unreadForYou?: number } | null | undefined,
): number | undefined {
  if (!payload) return undefined
  if (typeof payload.unreadForYou === 'number') return payload.unreadForYou
  return typeof payload.unread === 'number' ? payload.unread : undefined
}

export interface BadgeReporter {
  /** The page's state now: its cached unread count (undefined before the
   *  first read) and whether the session says nobody is signed in. */
  update(unread: number | undefined, signedOut: boolean): void
  stop(): void
}

export function createBadgeReporter(
  send: (count: number) => void,
  delay = BADGE_DEBOUNCE_MS,
): BadgeReporter {
  let reported: number | undefined
  let timer: ReturnType<typeof setTimeout> | undefined
  return {
    update(unread, signedOut) {
      const count = signedOut ? 0 : unread
      if (typeof count !== 'number') return
      // A pending report of an older count goes either way.
      clearTimeout(timer)
      timer = undefined
      if (count === reported) return
      timer = setTimeout(() => {
        timer = undefined
        reported = count
        send(count)
      }, delay)
    },
    stop() {
      clearTimeout(timer)
      timer = undefined
    },
  }
}
