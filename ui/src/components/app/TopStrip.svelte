<script lang="ts" module>
  import type { AppManifest } from '@/lib/apps'
  import { NAV } from '@/lib/nav'
  import { activeAmong, isUnder } from '@/lib/route-tabs'

  // Derive the active view's name from the route: nav items first (covers WORK/
  // MANAGE/SYSTEM and their subpaths), then enabled-app surfaces, then a plain
  // capitalization of the first path segment (e.g. /chat → Chat).
  function viewName(pathname: string, apps: AppManifest[]): string {
    if (pathname === '/' || isUnder(pathname, '/home')) return 'Inbox'
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
</script>

<script lang="ts">
  import UserMenu from './UserMenu.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import type { SessionUser } from '@/lib/session'
  import { route } from '@/router'

  // Mercury top strip (spec §6): breadcrumb readout, compact search
  // affordance, and the account chip. The brand lives in the sidebar now —
  // this strip is pure chrome on the ground surface.
  let { user, onLogout }: { user: SessionUser; onLogout: () => void } = $props()

  // Keep the query, default off it. Flattening to `[]` on the line that
  // created it puts `isError` permanently out of reach, and this breadcrumb
  // would then be unable to tell "no apps are enabled" from "the app manifest
  // read failed" — it falls back to the raw slug below, which is honest only
  // because the failure is still reachable from here.
  const appsQuery = useEnabledApps()
  const apps = $derived(appsQuery.data ?? [])
</script>

<!-- z-40: the strip (and the user-menu flyover inside it) stacks above all
     page content and its popovers; modals portal to body at z-50. -->
<header class="relative z-40 flex h-12 shrink-0 items-center justify-between gap-3 border-b border-line bg-surface px-4">
  <!-- Spec §6: the whole breadcrumb (including <VIEW>) is 10px mono
       uppercase 0.08em muted — no emphasized segment. -->
  <div class="min-w-0 truncate font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
    <span>Local Command Surface</span>
    <span class="mx-1.5 text-ink-dim">/</span>
    <span>{viewName(route.pathname, apps)}</span>
  </div>

  <div class="flex shrink-0 items-center gap-3">
    <!-- Spec §6 shows a `⌕ SEARCH ⌘K` tile, but Talaria has no global
         search / command palette yet — rendering the tile (or a ⌘K hint)
         would be a fabricated affordance (spec §7: no fabricated data).
         Add it here when a real search ships. -->
    <UserMenu {user} {onLogout} />
  </div>
</header>
