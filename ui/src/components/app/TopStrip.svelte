<script lang="ts" module>
  import type { AppManifest } from '@/lib/apps'
  import { NAV } from '@/lib/nav'
  import { activeAmong, isUnder } from '@/lib/route-tabs'

  // The section the view lives in — the one breadcrumb segment the strip keeps
  // (Manage, System), dropped entirely for work views: with the title gone the
  // dock tile you clicked already says where you are, and a second voice
  // repeating it reads as noise, not wayfinding.
  function viewSection(pathname: string, apps: AppManifest[]): string | undefined {
    if (pathname === '/' || isUnder(pathname, '/home')) return undefined
    if (pathname === '/settings' || isUnder(pathname, '/settings')) return 'System'
    if (pathname === '/admin' || isUnder(pathname, '/admin')) return 'System'
    const appMatch = /^\/x\/([^/]+)/.exec(pathname)
    if (appMatch) {
      const app = apps.find((a) => a.slug === appMatch[1])
      // Manage surfaces still breadcrumb their section; app work surfaces sit
      // among the other Work views, which carry no section label.
      return app && isUnder(pathname, `/x/${app.slug}/manage`) ? 'Manage' : undefined
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
  import BoardsSearch from './BoardsSearch.svelte'
  import type { SessionUser } from '@/lib/session'
  import { route } from '@/router'
  // `isUnder` is already in the module scope above; the instance script reads
  // it from there (Svelte 5 module scripts share their imports downward).
  const onBoards = (path: string): boolean => isUnder(path, '/boards')

  // Mercury top strip (spec §6), the dock era: no title row, no copy-link —
  // the dock names the view, the view owns its whole height, and link/copy
  // affordances return later integrated into each view (the ticket defers
  // them deliberately). The strip is pure personal chrome: search, the bell,
  // the account chip — the two things every view shares, right of everything.
  let { user, onLogout }: { user: SessionUser; onLogout: () => void } = $props()

  const section = $derived(viewSection(route.pathname, []))
  // The boards view carries its own sidebar (its own search lives in the
  // board's toolbar), so the global box stays out of the way there.
  const onBoardsHere = $derived(onBoards(route.pathname))
</script>

<!-- z-40: the strip (and the flyovers inside it) stacks above all page
     content and its popovers; modals portal to body at z-50. -->
<header class="relative z-40 flex shrink-0 items-center justify-between gap-3 border-b border-line bg-surface px-4 py-2">
  <div class="flex min-w-0 items-center gap-1.5">
    {#if section}
      <div class="truncate font-mono text-[10px] uppercase tracking-[0.08em] text-muted">{section}</div>
    {/if}
  </div>

  <div class="flex shrink-0 items-center gap-3">
    {#if !onBoardsHere}
      <BoardsSearch />
    {/if}
    <!-- The bell before the account chip: what is waiting for you, then who
         you are. -->
    <NotificationBell />
    <UserMenu {user} {onLogout} />
  </div>
</header>
