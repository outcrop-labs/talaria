// THE ONE WAY A NOTIFICATION IS OPENED from the page: a toast click, the
// service worker relaying an OS banner click, the desktop shell's
// `talaria:notification-open` event, and the `new Notification` fallback all
// land here. Three things, in this order:
//
//   1. The href is checked: only a path on THIS origin opens (a leading `/`,
//      not `//`, no backslash, same origin once resolved). Anything else goes
//      to `/` — a notification is never a way off the instance.
//   2. The app routes there IN PLACE (navigateHref, which refuses another
//      origin on its own) — no full page load, no lost state.
//   3. The row is marked read, and the bell, the rail badges and Home
//      refetch. Opening a notification is reading it; before this every click
//      path left the row unread and the badge counting it.

/** The in-app path an href may open, or null. Pure. The same rule the
 *  server stores by (talaria-notify `openable_href`) and the shell opens by. */
export function safeInAppHref(href: string | null | undefined, origin: string): string | null {
  const h = (href ?? '').trim()
  if (!h || h.length > 2048 || !h.startsWith('/') || h.startsWith('//')) return null
  // Controls and backslashes: `/\evil.example` is another host to some engines.
  for (let i = 0; i < h.length; i++) {
    const c = h.charCodeAt(i)
    if (c < 0x20 || c === 0x7f || c === 0x5c) return null
  }
  try {
    const u = new URL(h, origin)
    if (u.origin !== new URL(origin).origin) return null
    return u.pathname + u.search + u.hash
  } catch {
    return null
  }
}

export interface OpenTarget {
  id: string
  href?: string | null
}

export interface OpenDeps {
  /** In-app navigation (navigateHref). */
  navigate: (path: string) => Promise<unknown> | unknown
  /** Mark these rows read (PUT /api/notifications {ids}). */
  markRead: (ids: string[]) => Promise<unknown>
  /** Refetch what the read changes: notifications, unreads, home. */
  invalidate: () => Promise<unknown> | unknown
  origin: string
}

/** Open one notification: route in place, then mark it read. Navigation does
 *  not wait on the write — the person clicked to GO there. Answers the path
 *  it routed to. */
export async function openNotification(target: OpenTarget, deps: OpenDeps): Promise<string> {
  const path = safeInAppHref(target.href, deps.origin) ?? '/'
  const nav = Promise.resolve()
    .then(() => deps.navigate(path))
    .catch(() => {
      /* a refused navigation leaves the person where they were */
    })
  if (target.id) {
    try {
      await deps.markRead([target.id])
    } catch {
      /* the bell still offers the row; the poll reconciles */
    }
    await Promise.resolve(deps.invalidate()).catch(() => {})
  }
  await nav
  return path
}
