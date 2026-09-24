// The nav rail's section derivation, lifted out of NavRail.svelte so the
// coming top dock and the manage sidebar build on the same logic — and so the
// logic can be tested at all. This file is PURE: inputs are plain arguments,
// the output is plain data, and nothing here knows Svelte exists. The
// component keeps only what is genuinely reactive (the apps query and its
// broken/stale state, the live pathname, the session's grants).
//
// WHY `navSections` TAKES THE CORE SECTIONS AS AN ARGUMENT rather than
// importing NAV itself: NAV's items carry Lucide icons, and @lucide/svelte's
// icon modules are raw `.svelte` files, so importing NAV from a node-env
// vitest test fails on `Unknown file extension ".svelte"` — and the ui suite
// is node-env by design (see vitest.config.ts, which also documents why
// nothing may loosen that). The type imports below are erased at runtime and
// cost nothing. Callers hand over NAV (tests hand over a fixture shaped like
// it); the derivation never needed to own it. Everything it branches on is
// the sections' stable `id`s — 'work', 'manage', 'apps' — never a display
// string, which the Work section doesn't even have.

import { activeAmong } from '@/lib/route-tabs'
import type { NavItem, NavSection } from '@/lib/nav'

/** An app's injected nav entry: a manifest's work or manage surface turned
 *  into a nav item. `icon` is the manifest's string glyph — renderers handle
 *  Lucide components and strings both (see NavItem.icon). */
export type AppNavItem = NavItem & { icon: string }

/** An app's WORK surface as a nav entry, or null when the app publishes no
 *  work surface. */
export function appWorkItem(app: AppManifestLike): AppNavItem | null {
  const work = app.surfaces.work
  return work ? { to: `/x/${app.slug}`, label: work, icon: app.icon } : null
}

/** An app's MANAGE surface as a nav entry, or null when the app publishes no
 *  manage surface. Manage is the control plane no matter who published the
 *  view — same grant model as any core Manage view. */
export function appManageItem(app: AppManifestLike): AppNavItem | null {
  const manage = app.surfaces.manage
  return manage ? { to: `/x/${app.slug}/manage`, label: manage, icon: app.icon } : null
}

/** The manifest fields the derivation reads. Structural, so tests can build
 *  plain fixtures without importing the apps module (whose types pull the
 *  same node-env-unfriendly imports NAV does — only as types here, which
 *  erase). */
export interface AppManifestLike {
  slug: string
  icon: string
  surfaces: { work?: string; manage?: string }
}

/**
 * Derive the nav's sections from the core menu plus the enabled apps' two
 * surfaces, under the viewer's grants.
 *
 * Rules, verbatim from the rail:
 *  · app MANAGE surfaces append to the core section whose stable `id` is
 *    'manage';
 *  · app WORK surfaces form their own 'apps' section — Work is Talaria's own
 *    surfaces, and an app's work surface is a guest with its own heading —
 *    placed after the Work views when they exist, otherwise ahead of Manage
 *    (or at the top, in the nothing-core-survives edge): views, Apps,
 *    Manage, either way;
 *  · the Apps section appears only when it has something in it — no app
 *    installed, no empty heading;
 *  · denied and role-gated items are dropped, and a section left with
 *    nothing is dropped with them.
 */
export function navSections(
  core: NavSection[],
  appWork: AppNavItem[],
  appManage: AppNavItem[],
  opts: { isAdmin: boolean; denied: readonly string[] },
): NavSection[] {
  const { isAdmin, denied } = opts

  // Denied-view + role filtering, shared by both modes and by the app items
  // wherever they land.
  const passes = (i: NavItem) =>
    (!i.adminOnly || isAdmin) && !denied.includes(i.to) && !denied.some((d) => i.to.startsWith(d + '/'))

  const merged = core.flatMap((section) => {
    if (section.adminOnly && !isAdmin) return []
    const items = [...section.items, ...(section.id === 'manage' ? appManage : [])].filter(passes)
    return items.length === 0 ? [] : [{ id: section.id, title: section.title, items }]
  })

  const apps = appWork.filter(passes)
  if (apps.length === 0) return merged
  // After the Work views when they exist; otherwise ahead of Manage (or at
  // the top, in the nothing-core-survives edge). Placement branches on the
  // section's stable `id`, never its display string: the Work views carry no
  // header to match on.
  const workAt = merged.findIndex((s) => s.id === 'work')
  const at = workAt >= 0 ? workAt + 1 : Math.max(merged.findIndex((s) => s.id === 'manage'), 0)
  return [...merged.slice(0, at), { id: 'apps', title: 'Apps', items: apps }, ...merged.slice(at)]
}

/**
 * The ACTIVE item is the MOST SPECIFIC one containing the route, decided
 * across every section at once by `activeAmong` — so `/boards` stays lit
 * while you read a task inside it, and an app's Manage surface beats its Work
 * surface instead of lighting both. (See route-tabs.ts for the rule's whole
 * history: it replaced the rail's `exactFor`, which matched two cases and
 * stated no rule.)
 */
export function navActiveItem(pathname: string, sections: readonly NavSection[]): NavItem | null {
  const activePath = activeAmong(pathname, sections.flatMap((sec) => sec.items.map((i) => i.to)))
  if (activePath === null) return null
  for (const sec of sections) {
    const item = sec.items.find((i) => i.to === activePath)
    if (item) return item
  }
  return null
}