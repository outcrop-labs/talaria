<script lang="ts" module>
  import type { SessionUser } from '@/lib/session'
  import { navSections, navActiveItem, appWorkItem, appManageItem } from '@/lib/nav-sections'
  import { NAV } from '@/lib/nav'

  function dockSections(
    user: SessionUser,
    appsRows: { slug: string; icon: string; surfaces: { work?: string; manage?: string } }[],
    grants: { denied: readonly string[]; appGrants: readonly string[] | null },
  ) {
    const appWork = appsRows.flatMap((a) => {
      const item = appWorkItem(a)
      return item === null ? [] : [item]
    })
    const appManage = appsRows.flatMap((a) => {
      const item = appManageItem(a)
      return item === null ? [] : [item]
    })
    const sections = navSections(NAV, appWork, appManage, { isAdmin: user.role === 'admin', ...grants })
    return {
      row: sections.filter((s) => s.id !== 'manage'),
      manage: sections.find((s) => s.id === 'manage') ?? null,
    }
  }
</script>

<script lang="ts">
  import { untrack } from 'svelte'
  import { ChevronLeft, Cog, TriangleAlert } from '@lucide/svelte'
  import WingMark from '@/components/WingMark.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import type { NavItem } from '@/lib/nav'
  import TabFace from './TabFace.svelte'
  import RailTooltip from './RailTooltip.svelte'
  import DesktopSwitcher from './DesktopSwitcher.svelte'
  import DesktopWindowControls from './DesktopWindowControls.svelte'
  import SidebarAssistant from './SidebarAssistant.svelte'
  import BoardsSearch from './BoardsSearch.svelte'
  import NotificationBell from './NotificationBell.svelte'
  import UserMenu from './UserMenu.svelte'
  import { isUnder } from '@/lib/route-tabs'
  import { badgeTitle, useNavBadges } from '@/lib/nav-badges.svelte'
  import { useEnabledApps } from '@/lib/apps'
  import { useAppGrants, useDeniedViews } from '@/lib/session'
  import { MANAGE_HOME } from '@/lib/manage-area'
  import { settingsNav } from '@/lib/settings-nav'
  import { backToApp, rememberOutside } from '@/lib/settings-return'
  import { edgeFade, revealCurrent } from '@/lib/tab-strip'
  import { readout, tabWorking } from '@/lib/tab-working'
  import { tabMarkOnScreen } from '@/lib/tab-mark-on-screen'
  import { useWorking } from '@/lib/working.svelte'
  import { swap } from '@/lib/motion'
  import { windowDrag } from '@/lib/desktop-titlebar'
  import { tabLift } from '@/lib/tab-motion'
  import { cn } from '@/lib/cn'
  import { p, route } from '@/router'

  let { user, onLogout }: { user: SessionUser; onLogout: () => void } = $props()

  const denied = useDeniedViews()
  const appGrants = useAppGrants()
  const badges = useNavBadges(() => route.pathname)
  const working = useWorking()

  const appsQuery = useEnabledApps()
  const appsList = listQuery(appsQuery, { title: 'App links unavailable', variant: 'inline' })
  const appsBroken = $derived(appsList.failed || appsList.stale)

  const boardsHasOwnSearch = $derived(isUnder(route.pathname, '/boards'))

  const grants = $derived({ denied: denied.current, appGrants: appGrants.current })
  const { row, manage: manageSection } = $derived(dockSections(user, appsList.rows, grants))
  // The sidebar's own model decides what "inside the settings area" means, so
  // the dock gives way on exactly the routes where the sidebar shows.
  const area = $derived(settingsNav(NAV, appsList.rows, { isAdmin: user.role === 'admin', ...grants }, route.pathname))
  const hasManage = $derived(area.groups.some((g) => g.id === 'workspace'))

  let lastOutside = $state<string | null>(null)
  $effect(() => {
    // Read for the dependency: a query-only move is still a new address.
    void route.search
    const href = route.pathname + window.location.search + route.hash
    const inside = area.inside
    lastOutside = rememberOutside(untrack(() => lastOutside), href, inside)
  })
  const activeCandidates = $derived([...row, ...(manageSection ? [manageSection] : [])])
  const activePath = $derived(navActiveItem(route.pathname, activeCandidates)?.to ?? null)
  const workingElsewhere = $derived(readout(row.flatMap((s) => s.items), working.current, route.pathname))
</script>

{#snippet face(item: NavItem, size: number)}
  <TabFace icon={item.icon} {size} site={tabWorking(item.to, working.current)?.site ?? null} tab />
{/snippet}

{#snippet badgeMark(badge: number | null, className: string)}
  <span
    title={badgeTitle(badge)}
    class={cn(
      'font-mono text-[10px] leading-3 tracking-[0.05em]',
      badge === null ? 'text-[color:var(--theme-danger)]' : 'text-muted',
      className,
    )}
  >
    {badge === null ? '!' : badge}
  </span>
{/snippet}

<!-- h-14 is shell chrome, not a stage header, so it does not join the h-12
     stage-header line (UI-CONVENTIONS). -->
<nav aria-label="Primary" class="@container relative z-40 flex h-14 shrink-0 items-stretch gap-2 bg-sidebar px-2" use:windowDrag>
  <!-- A border on the nav would paint over the open tab. -->
  <div class="pointer-events-none absolute inset-x-0 bottom-0 h-px bg-line" aria-hidden="true"></div>

  <div class="flex shrink-0 items-center gap-2">
    <DesktopWindowControls at="left" />
    <div class="flex h-9 shrink-0 items-center gap-2.5 pl-1" aria-label="Talaria">
      <WingMark size={28} />
      <span
        class="hidden font-sans text-[13px] font-semibold leading-4 tracking-[0.18em] text-fg @4xl:block"
        aria-hidden="true">TALARIA</span
      >
    </div>
    <DesktopSwitcher collapsed />
  </div>

  {#if area.inside}
    <!-- Settings is its own mode: its sidebar is the navigation, so the tab
         strip gives way to the one link that leaves. -->
    <div class="flex min-w-0 flex-1 items-center px-3">
      <a
        href={backToApp(lastOutside)}
        aria-label="Back to app"
        class="flex h-8 shrink-0 items-center gap-1.5 rounded-md pl-1.5 pr-2.5 font-sans text-[13.5px] leading-5 text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg"
      >
        <ChevronLeft size={16} strokeWidth={1.5} class="shrink-0" />
        <span>Back<span class="hidden @lg:inline">&nbsp;to app</span></span>
      </a>
    </div>
  {:else}
  <div class="tab-strip relative flex min-w-0 flex-1 items-end gap-2 overflow-x-auto px-3" use:edgeFade use:revealCurrent use:tabLift={route.pathname}>
    {#each row as section, si (section.id)}
      {#if si > 0}<div class="mb-3 h-5 w-px shrink-0 bg-line"></div>{/if}
      {#each section.items as item (item.to)}
        {@const badge = badges.badgeFor(item)}
        {#if item.to === activePath}
          <a
            href={item.to}
            aria-label={item.label}
            aria-current="page"
            class="ears-bottom flex h-11 min-w-0 max-w-[15rem] shrink-0 items-center gap-2 rounded-t-[10px] border border-b-0 border-line bg-surface pl-3 pr-4 text-fg focus-visible:outline-offset-[-2px]"
          >
            {@render face(item, 18)}
            <span class="truncate font-sans text-[14px] font-medium leading-[1.25]">{item.label}</span>
            {#if badge !== undefined && (badge === null || badge > 0)}{@render badgeMark(badge, 'shrink-0')}{/if}
          </a>
        {:else}
          <RailTooltip label={item.label}>
            <a
              href={item.to}
              aria-label={item.label}
              class="tab-tile relative mb-1.5 grid h-8 w-8 shrink-0 place-items-center rounded-md text-muted dither-fill hover:text-fg"
            >
              {@render face(item, 16)}
              {#if badge !== undefined && (badge === null || badge > 0)}{@render badgeMark(badge, 'absolute right-0 top-0')}{/if}
            </a>
          </RailTooltip>
        {/if}
      {/each}
    {/each}

    {#if appsList.notice}
      <div class="mb-1.5 shrink-0 self-end"><QueryError {...appsList.notice} /></div>
    {/if}
  </div>
  {/if}

  <div class="flex shrink-0 items-center gap-2">
    <!-- Settings is where things are configured, not watched: its dock keeps
         the way out, search, the bell and the account, and nothing live. -->
    {#if workingElsewhere && !area.inside}
      {@const w = workingElsewhere}
      <a
        href={w.lead.href}
        title={w.title}
        aria-label="{w.elsewhere} working. {w.title.replaceAll('\n', '. ')}"
        class="flex h-7 max-w-[18rem] shrink-0 items-center gap-1.5 rounded-md border border-line px-2 text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
        in:swap={{ duration: 200 }}
        out:swap={{ duration: 120 }}
      >
        <TabFace icon={w.tab.icon} size={12} site={w.site} still={tabMarkOnScreen(w.site)} />
        <span class="font-mono text-[10px] uppercase leading-3 tracking-[0.08em]"
          >{w.elsewhere}<span class="hidden pl-1 @2xl:inline">working</span></span
        >
        <span class="hidden min-w-0 truncate font-sans text-[12px] leading-4 text-fg @5xl:block">{w.lead.label}</span>
      </a>
    {/if}
    {#if appsBroken}
      <RailTooltip label="App links unavailable; retry">
        <button
          type="button"
          onclick={() => void appsQuery.refetch()}
          aria-label="App links unavailable; retry"
          class="grid h-9 w-9 place-items-center rounded-md text-danger transition-colors duration-[120ms] dither-fill"
        >
          <TriangleAlert size={16} strokeWidth={1.5} />
        </button>
      </RailTooltip>
    {/if}
    {#if !area.inside}<SidebarAssistant collapsed />{/if}
    {#if hasManage && !area.inside}
      <RailTooltip label="Manage">
        <a
          href={p(MANAGE_HOME)}
          aria-label="Manage"
          class="relative grid h-9 w-9 shrink-0 place-items-center rounded-md text-muted transition-colors duration-[120ms] dither-fill hover:text-fg"
        >
          <Cog size={16} strokeWidth={1.5} />
        </a>
      </RailTooltip>
    {/if}
    {#if !area.inside || appsBroken}<div class="my-2 h-6 w-px shrink-0 bg-line"></div>{/if}
    {#if !boardsHasOwnSearch}
      <BoardsSearch />
    {/if}
    <NotificationBell />
    <UserMenu {user} {onLogout} {hasManage} voice={area.inside ? 'sans' : 'mono'} />
    <DesktopWindowControls at="right" />
  </div>
</nav>
