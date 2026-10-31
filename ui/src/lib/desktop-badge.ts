// The dock and launcher badge reporter (interface I-5): one instance page's
// unread count, handed to the desktop shell, which keeps a share per instance
// and shows the sum. Debounced, so a burst of refetches is one report — of the
// count the burst SETTLED on: 2 → 3 → 2 inside the window reports nothing new,
// not 3. A signed-out page reports 0, because the query cache still holds the
// last signed-in count and the login screen mounts the switcher too.

export const BADGE_DEBOUNCE_MS = 500

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
