// NOTHING READS THIS MODULE TODAY. It existed so the top strip could title
// the view: the strip rendered ABOVE the outlet and could not reach into the
// view to ask, so the view CLAIMED its title on mount (ViewHeader does it for
// views that keep body chrome — status, actions, a blurb; views whose header
// was only a title call claimViewTitle directly) and the strip read the
// claim. The dock era dropped the title row, and the strip itself is now
// deleted, so `viewTitleClaim` has no caller anywhere in the app.
//
// It is kept rather than deleted because the claims are still made, correctly
// and cheaply, by a dozen views — and the mechanism below is the part that
// was hard to get right (see the keying note). Whatever names views next
// reads `viewTitleClaim` and inherits a working surface. If the answer turns
// out to be "nothing ever does", this module and its callers go together, in
// one change that can say so.
//
// The claim is keyed by the pathname it was made under, and that key is the
// whole correctness story. During a route change the outgoing view's claim is
// still sitting in this module when the strip re-renders for the new path; an
// unkeyed "last claim wins" would title the new view with the old one's words
// for a frame — or for good, on a surface that claims nothing. A claim whose
// path is not the current path is simply never read: no teardown ordering to
// get right, no cleanup to forget.

import { route } from '@/router'

export interface ViewTitleExtras {
  /** InfoTip copy shown beside the title. */
  info?: string
  /** Breadcrumb segments after the section/view pair — the named thing's
   *  place in the world, outermost first (e.g. team, then board). */
  trail?: string[]
}

let claim = $state<{ path: string; title: string } & ViewTitleExtras | null>(null)

/** Record the calling view's strip title and optional extras. Call during
 *  the view's initial render — the strip and the view mount in the same
 *  flush, so the title is present on the first painted frame rather than
 *  flashing the route-derived fallback — or from an `$effect` when the
 *  titled thing arrives from a query (claim again when it lands; the key
 *  keeps interim claims honest). */
export function claimViewTitle(title: string, extras?: ViewTitleExtras): void {
  claim = { path: route.pathname, title, ...extras }
}

/** The title claimed for THIS path, if any. `null` means the strip falls
 *  back to the route-derived view name. */
export function viewTitleClaim(pathname: string): { title: string } & ViewTitleExtras | null {
  return claim?.path === pathname ? claim : null
}
