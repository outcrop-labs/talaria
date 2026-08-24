// The top strip titles the view — but the strip renders ABOVE the outlet and
// cannot reach into the view to ask. So the view CLAIMS its title on mount
// (ViewHeader does it for views that keep body chrome — status, actions, a
// blurb; views whose header was only a title call claimViewTitle directly),
// and the strip reads the claim, falling back to the route-derived name for
// the full-bleed surfaces that never had a body title to move.
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
