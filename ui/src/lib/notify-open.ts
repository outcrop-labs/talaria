// THE ONE WAY A NOTIFICATION IS OPENED from the page: a toast click, the
// service worker relaying an OS banner click, the desktop shell's
// `talaria:notification-open` event, and the `new Notification` fallback all
// land here. Three things, in this order:
//
//   1. The href is checked: only a path on THIS origin opens (a leading `/`,
//      not `//`, no backslash, no `.`/`..` segment, same origin once
//      resolved AND once that path is resolved again). Anything else opens
//      nothing — a notification is never a way off the instance.
//   2. The app routes there IN PLACE (navigateHref, which refuses another
//      origin on its own) — no full page load, no lost state. A row with no
//      link (an empty href, or one the rule refused) leaves the person where
//      they are; it is still read.
//   3. The row is marked read, and the bell, the rail badges and Home
//      refetch. Opening a notification is reading it; before this every click
//      path left the row unread and the badge counting it.

/** Does the PATH part (before `?` or `#`) hold a segment a URL parser
 *  resolves away — `.` or `..`, in any `%2e` spelling, any case? Such a path
 *  can resolve to `//host`: `/..//evil.example` is the path `//evil.example`
 *  on this origin, and that path parsed again is another host. */
export function hasDotSegment(h: string): boolean {
  const path = h.split(/[?#]/, 1)[0] ?? ''
  return path.split('/').some((seg) => {
    const s = seg.toLowerCase().replaceAll('%2e', '.')
    return s === '.' || s === '..'
  })
}

/** The in-app path an href may open, or null. Pure. The same rule the
 *  server stores by (talaria-notify `openable_href`) and the service
 *  worker opens by (sw.ts `safeHref`, an inlined copy). */
export function safeInAppHref(href: string | null | undefined, origin: string): string | null {
  const h = (href ?? '').trim()
  if (!h || h.length > 2048 || !h.startsWith('/') || h.startsWith('//')) return null
  // Controls and backslashes: `/\evil.example` is another host to some engines.
  for (let i = 0; i < h.length; i++) {
    const c = h.charCodeAt(i)
    if (c < 0x20 || c === 0x7f || c === 0x5c) return null
  }
  if (hasDotSegment(h)) return null
  try {
    const home = new URL(origin).origin
    const u = new URL(h, origin)
    if (u.origin !== home) return null
    const path = u.pathname + u.search + u.hash
    // The path is what every caller hands to a parser next: it must stay on
    // this origin THEN too (belt to the dot-segment braces above).
    if (path.startsWith('//') || new URL(path, origin).origin !== home) return null
    return path
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

/** A notification row's id (a uuid). Anything else — the catch-up summary's
 *  `talaria-summary` tag, the "Desktop alerts are on" banner's empty id — is
 *  not a row, and a mark-read of it would answer 400. */
const ROW_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
export const isRowId = (id: string | null | undefined): id is string => !!id && ROW_ID.test(id)

/** Open one notification: route in place, then mark it read. Navigation does
 *  not wait on the write — the person clicked to GO there. A notification
 *  with no openable link (empty, or refused by the rule) routes nowhere: the
 *  person stays on the page they are on. Answers the path it routed to, or
 *  null when it stayed. */
export async function openNotification(target: OpenTarget, deps: OpenDeps): Promise<string | null> {
  const path = safeInAppHref(target.href, deps.origin)
  const nav = path
    ? Promise.resolve()
        .then(() => deps.navigate(path))
        .catch(() => {
          /* a refused navigation leaves the person where they were */
        })
    : Promise.resolve()
  if (isRowId(target.id)) {
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
