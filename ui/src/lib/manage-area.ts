// The manage area: which manage views this viewer has, and whether the route
// is inside one of them. The dock's gear and the manage sidebar both ask, so
// the answer lives here once. PURE, like nav-sections.ts which it builds on:
// the core sections arrive as an argument (NAV's Lucide icons are .svelte
// modules the node-env tests cannot import), and nothing here knows Svelte.

import type { NavItem, NavSection } from '@/lib/nav'
import { appManageItem, navActiveItem, navSections, type AppManifestLike } from '@/lib/nav-sections'
import { isUnder } from '@/lib/route-tabs'
import type { ViewerGrants } from '@/lib/app-grants'

/** The overview's address. Not a grantable view: it is the manage area's
 *  front door, open to whoever has at least one manage view behind it. */
export const MANAGE_HOME = '/manage'

export interface ManageArea {
  /** The manage views this viewer can reach — core first, then each enabled
   *  app's manage surface — after grants and role gates. Empty for a member
   *  with no manage grants: no gear, no sidebar, an overview with nothing in
   *  it. */
  items: NavItem[]
  /** The row that is "here": `/manage` on the overview, the most specific
   *  manage view containing the route elsewhere, null outside the area. */
  activePath: string | null
  /** Whether the route is in the manage area at all — the sidebar shows and
   *  the gear lights exactly when this is true. */
  inside: boolean
}

export function manageArea(
  core: NavSection[],
  apps: readonly AppManifestLike[],
  opts: ViewerGrants,
  pathname: string,
): ManageArea {
  const appManage = apps.flatMap((a) => {
    const item = appManageItem(a)
    return item === null ? [] : [item]
  })
  const section = navSections(core, [], appManage, opts).find((s) => s.id === 'manage') ?? null
  const items = section?.items ?? []
  if (items.length === 0) return { items, activePath: null, inside: false }
  if (isUnder(pathname, MANAGE_HOME)) return { items, activePath: MANAGE_HOME, inside: true }
  const activePath = section ? (navActiveItem(pathname, [section])?.to ?? null) : null
  return { items, activePath, inside: activePath !== null }
}
