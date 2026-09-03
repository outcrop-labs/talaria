<script lang="ts" module>
  import type { AppManifest } from '@/lib/apps'
  import { NAV } from '@/lib/nav'
  import { activeAmong, isUnder, tabFromPath } from '@/lib/route-tabs'
  import { HOME_TABS } from '@/routes/app/home/home'

  // Derive the active view's name from the route: nav items first (covers WORK/
  // MANAGE/SYSTEM and their subpaths), then enabled-app surfaces, then a plain
  // capitalization of the first path segment (e.g. /chat → Chat).
  function viewName(pathname: string, apps: AppManifest[]): string {
    if (pathname === '/' || isUnder(pathname, '/home')) {
      // Home's tabs are views with real names — /home/boards is Home's digest
      // of Boards, not "Inbox". The rail item says Inbox because it points at
      // the CONTAINER; the strip is per-view, so it names the tab. `tabFromPath`
      // is the same call Home.svelte makes, so /home/nonsense titles as the
      // tab it actually renders.
      const tab = tabFromPath(pathname, '/home', HOME_TABS, 'inbox')
      return tab === 'inbox' ? 'Inbox' : tab.charAt(0).toUpperCase() + tab.slice(1)
    }
    const appMatch = /^\/x\/([^/]+)/.exec(pathname)
    if (appMatch) {
      const app = apps.find((a) => a.slug === appMatch[1])
      // `isUnder`, not a bare prefix: `/x/foo/managers` is not the manage
      // surface, and naming the wrong one in the top strip is the kind of
      // wrong that reads as a rendering glitch rather than a routing bug.
      if (app) return (isUnder(pathname, `/x/${app.slug}/manage`) ? app.surfaces.manage : app.surfaces.work) ?? app.name
      return appMatch[1]!
    }
    // THE SAME RULE THE RAIL USES, from the same function. This inlined its
    // own copy of the prefix test and took the FIRST match in nav order, so a
    // nested pair was named by whichever happened to be declared first while
    // the rail highlighted the other — the strip and the rail disagreeing about
    // where you are.
    const items = NAV.flatMap((s) => s.items).filter((i) => i.to !== '/')
    const active = activeAmong(pathname, items.map((i) => i.to))
    const item = active === null ? undefined : items.find((i) => i.to === active)
    if (item) return item.label
    const seg = pathname.split('/').filter(Boolean)[0] ?? ''
    return seg ? seg.charAt(0).toUpperCase() + seg.slice(1) : 'Inbox'
  }

  // The section the view lives in — the leading breadcrumb segment now that
  // the view's own name leads as the title. Settings and Admin are not in NAV
  // (they live in the user menu), but they are still the SYSTEM bucket of
  // spec §5, and a location line that goes quiet exactly there would read as
  // a gap, not as restraint.
  function viewSection(pathname: string, apps: AppManifest[]): string | undefined {
    if (pathname === '/' || isUnder(pathname, '/home')) return 'Work'
    if (pathname === '/settings' || isUnder(pathname, '/settings')) return 'System'
    if (pathname === '/admin' || isUnder(pathname, '/admin')) return 'System'
    const appMatch = /^\/x\/([^/]+)/.exec(pathname)
    if (appMatch) {
      const app = apps.find((a) => a.slug === appMatch[1])
      // Same manage/work split the rail uses when slotting app surfaces in.
      return app ? (isUnder(pathname, `/x/${app.slug}/manage`) ? 'Manage' : 'Work') : undefined
    }
    const items = NAV.flatMap((s) => s.items).filter((i) => i.to !== '/')
    const active = activeAmong(pathname, items.map((i) => i.to))
    if (active === null) return undefined
    return NAV.find((s) => s.items.some((i) => i.to === active))?.title
  }
</script>

<script lang="ts">
  import NotificationBell from './NotificationBell.svelte'
  import UserMenu from './UserMenu.svelte'
  import CopyButton from '@/components/ui/CopyButton.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import type { SessionUser } from '@/lib/session'
  import { route } from '@/router'
  import { viewTitleClaim } from '@/lib/view-title.svelte'

  // Mercury top strip (spec §6): the view's title and the location breadcrumb
  // beneath it on the left, the account chip on the right. The brand lives in
  // the sidebar now — this strip is pure chrome on the ground surface.
  //
  // THE TITLE IS THE VIEW'S OWN. Views used to open with an in-body heading;
  // that row moved up here, claimed by the view on mount (ViewHeader does it
  // for the views that keep body chrome). Surfaces that never had one — the
  // full-bleed work views — fall back to the route-derived name, so every
  // view is titled from the same place the breadcrumb always named it.
  let { user, onLogout }: { user: SessionUser; onLogout: () => void } = $props()

  // Keep the query, default off it. Flattening to `[]` on the line that
  // created it puts `isError` permanently out of reach, and this breadcrumb
  // would then be unable to tell "no apps are enabled" from "the app manifest
  // read failed" — it falls back to the raw slug below, which is honest only
  // because the failure is still reachable from here.
  const appsQuery = useEnabledApps()
  const apps = $derived(appsQuery.data ?? [])

  const claim = $derived(viewTitleClaim(route.pathname))
  const view = $derived(viewName(route.pathname, apps))
  const section = $derived(viewSection(route.pathname, apps))
</script>

<!-- z-40: the strip (and the user-menu flyover inside it) stacks above all
     page content and its popovers; modals portal to body at z-50. -->
<header class="relative z-40 flex shrink-0 items-center justify-between gap-3 border-b border-line bg-surface px-4 py-2">
  <div class="min-w-0">
    <div class="flex items-center gap-1.5">
      <h1 class="truncate font-sans text-base font-semibold tracking-tight text-fg">{claim?.title ?? view}</h1>
      {#if claim?.info}<InfoTip text={claim.info} />{/if}
    </div>
    <!-- Spec §6 breadcrumb voice, kept as the location line under the title:
         10px mono uppercase 0.08em muted, section / view (then the claimed
         trail — the thing's place in the world), no emphasized segment. -->
    <div class="mt-0.5 flex min-w-0 items-center gap-1">
      <div class="truncate font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
        {#if section}{section}<span class="mx-1.5 text-ink-dim">/</span>{/if}{view}{#each claim?.trail ?? [] as seg}<span class="mx-1.5 text-ink-dim">/</span>{seg}{/each}
      </div>
      <!-- The copy-link that rounds the line off: the crumb names where you
           are, this hands that place to someone. Search rides along (a board
           on its gantt view is a different link than the board), and the
           pathname re-read re-fetches it; p-0.5/-my-1 keep a 10px line from
           growing around a 13px icon. -->
      <CopyButton path={route.pathname + window.location.search} title="Copy link" class="shrink-0 -my-1 p-0.5" />
    </div>
  </div>

  <div class="flex shrink-0 items-center gap-3">
    <!-- Spec §6 shows a `⌕ SEARCH ⌘K` tile, but Talaria has no global
         search / command palette yet — rendering the tile (or a ⌘K hint)
         would be a fabricated affordance (spec §7: no fabricated data).
         Add it here when a real search ships. -->
    <!-- The bell before the account chip: what is waiting for you, then who
         you are — the two things every view shares, right of every title. -->
    <NotificationBell />
    <UserMenu {user} {onLogout} />
  </div>
</header>
