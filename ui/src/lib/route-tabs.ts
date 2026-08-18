// Tabs that are LOCATIONS.
//
// Every tabbed view in the app used to keep its tab in `?tab=`, or in component
// state, which meant a tab was a mode rather than a place: you could not link
// to one, the back button walked past it, and "open the pricing tab" was a
// description of clicks instead of a URL. They are path segments now —
// `/models/pricing`, `/admin/security`, `/agents/schedules` — and this is the
// one rule they share.
//
// It lives here rather than in each view because the interesting half is the
// FALLBACK, and getting that wrong in six places would mean six different
// answers to "what does /models/nonsense do". One answer: the home tab. A
// typo'd or stale link lands on something real rather than an empty frame, and
// a tab removed in a later release keeps its old URLs working.

/**
 * Is this path inside `base` — the view itself, or anything under it?
 *
 * THE BUG THIS EXISTS FOR, because it presented as a broken nav rail rather
 * than as anything to do with routing. `route.pathname` flips the instant a nav
 * rail item is clicked, but the view being left is not destroyed until
 * afterwards, so for a beat its effects run against a URL that already belongs
 * to the next view. A view whose default-selection effect NAVIGATES — Comms
 * restoring your last channel, Knowledge canonicalising to a space — then reads
 * "I am open with nothing selected", does exactly what it was written to do, and
 * navigates back. The click is undone.
 *
 * It is a race against unmount, so it did not happen every time: the rail felt
 * dead and the same item wanted two or three clicks before it moved.
 *
 * So: any effect that navigates must first ask whether it is still on its own
 * view, and this is that question. One implementation rather than one per view,
 * because three hand-written copies of a predicate is how the copies drift —
 * and `base` matching must include the bare path (`/comms`) as well as the
 * children (`/comms/...`), which is the half an inlined `startsWith` forgets.
 */
export function isUnder(pathname: string, base: string): boolean {
  return pathname === base || pathname.startsWith(`${base}/`)
}

/** The tab a path selects, or `fallback` when the segment is missing, unknown,
 *  or deeper than one level.
 *
 *  `base` is the view's own path with no trailing slash ('/models'). */
export function tabFromPath<T extends string>(
  pathname: string,
  base: string,
  tabs: readonly T[],
  fallback: T,
): T {
  if (!pathname.startsWith(`${base}/`)) return fallback
  const rest = pathname.slice(base.length + 1)
  // One level only: `/models/pricing/extra` is not a tab, and treating it as
  // one would quietly render the pricing tab for a URL that means something
  // else (or nothing).
  if (!rest || rest.includes('/')) return fallback
  const seg = decodeURIComponent(rest)
  return (tabs as readonly string[]).includes(seg) ? (seg as T) : fallback
}

/** The single id a path selects, or null when the view is showing no selection.
 *
 *  The selection twin of `tabFromPath`, for views whose URL names ONE thing —
 *  /research/<runId>, /plan/<planId>. Same one-level rule: a deeper path is not
 *  a selection, because it means something this view does not define.
 *
 *  Ids are opaque here on purpose. A tab is drawn from a fixed set and can be
 *  validated against it; an id refers to a row that may have been deleted, and
 *  "does this exist" is a question for the query that loads it, which already
 *  has a not-found story. Validating shape here would only turn one honest
 *  empty state into two. */
export function pathId(pathname: string, base: string): string | null {
  if (!pathname.startsWith(`${base}/`)) return null
  const rest = pathname.slice(base.length + 1)
  if (!rest || rest.includes('/')) return null
  return decodeURIComponent(rest)
}
